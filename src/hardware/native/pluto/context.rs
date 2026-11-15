// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! What the radio's context XML says, read only as far as sdrtop uses it.
//!
//! `iiod` describes the whole radio in one XML document (`PRINT`): the
//! context's own attributes (model, serial, firmware), then every IIO device
//! with its channels and attribute names. sdrtop needs the context attributes
//! and the device names, and nothing here pretends to be a general XML parser:
//! it reads the elements libiio writes, in the form libiio writes them
//! (`iio_context_get_xml`: double-quoted attributes, the five standard
//! entities, no CDATA). A document of another shape reads as missing those
//! facts, and the radio is then refused for what is missing rather than
//! guessed about.
//!
//! **Entirely pure.** Text in, plain data out, so it is tested against the
//! document a PlutoSky R2 really sent.

/// The parts of a context description sdrtop reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// `<context-attribute name=".." value=".."/>`, in document order.
    pub attributes: Vec<(String, String)>,
    /// Each `<device>`'s `name`, or its `id` when it has no name.
    pub devices: Vec<String>,
}

impl Context {
    /// The value of a context attribute, when the radio reported one that is
    /// not blank.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.trim().is_empty())
    }

    /// Whether the radio lists an IIO device of this name.
    pub fn has_device(&self, name: &str) -> bool {
        self.devices.iter().any(|d| d == name)
    }
}

/// Reads a context description. Never fails: what cannot be read is absent.
pub fn parse(xml: &str) -> Context {
    let mut ctx = Context::default();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        let name_end = tag
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(tag.len());
        match &tag[..name_end] {
            "context-attribute" => {
                if let (Some(k), Some(v)) = (attr(tag, "name"), attr(tag, "value")) {
                    ctx.attributes.push((k, v));
                }
            }
            "device" => {
                if let Some(n) = attr(tag, "name").or_else(|| attr(tag, "id")) {
                    ctx.devices.push(n);
                }
            }
            _ => {}
        }
    }
    ctx
}

/// One double-quoted attribute of a tag's text, entities decoded.
fn attr(tag: &str, key: &str) -> Option<String> {
    let mut at = 0;
    while let Some(found) = tag[at..].find(key) {
        let start = at + found;
        let before_ok = start == 0 || tag[..start].ends_with(char::is_whitespace);
        let after = &tag[start + key.len()..];
        if before_ok {
            if let Some(value) = after.strip_prefix("=\"") {
                let end = value.find('"')?;
                return Some(unescape(&value[..end]));
            }
        }
        at = start + key.len();
    }
    None
}

/// The five entities libiio's XML writer produces.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document a PlutoSky R2 sent (`PRINT`, firmware v0.38, iiod 0.25),
    /// its serial replaced by one of the same length.
    const R2: &str = include_str!("../fixtures/pluto-r2-context.xml");

    #[test]
    fn the_r2_says_what_it_is() {
        let c = parse(R2);
        assert_eq!(
            c.attribute("hw_model"),
            Some("Analog Devices PlutoSDR Rev.C (Z7020-AD9361)")
        );
        assert_eq!(
            c.attribute("hw_serial"),
            Some("1044730231960000000000000000000001")
        );
        assert_eq!(c.attribute("fw_version"), Some("v0.38"));
        assert_eq!(c.attribute("ad9361-phy,model"), Some("ad9361"));
        assert_eq!(c.attribute("ad9361-phy,xo_correction"), Some("40000000"));
        assert_eq!(c.attribute("local,kernel"), Some("5.15.0"));
        assert_eq!(c.attributes.len(), 8);
    }

    #[test]
    fn the_r2_lists_its_four_devices() {
        let c = parse(R2);
        assert_eq!(
            c.devices,
            [
                "ad9361-phy",
                "xadc",
                "cf-ad9361-dds-core-lpc",
                "cf-ad9361-lpc"
            ]
        );
        assert!(c.has_device("cf-ad9361-lpc"));
        assert!(!c.has_device("cf-ad9361"));
    }

    #[test]
    fn entities_are_decoded_and_a_lookalike_key_is_not_taken() {
        let c = parse(
            r#"<context><context-attribute name="a&amp;b" value="&quot;x&lt;y&gt;&apos;" />
               <context-attribute xname="no" value="nope"/><device id="iio:device9" /></context>"#,
        );
        assert_eq!(c.attribute("a&b"), Some("\"x<y>'"));
        assert_eq!(c.attributes.len(), 1, "xname is not name");
        assert_eq!(
            c.devices,
            ["iio:device9"],
            "the id stands in for a missing name"
        );
    }

    #[test]
    fn what_cannot_be_read_is_absent() {
        assert_eq!(parse(""), Context::default());
        assert_eq!(
            parse("<context-attribute name=\"x\" value=\"")
                .attributes
                .len(),
            0
        );
        let blank = parse(r#"<context-attribute name="hw_serial" value=" " />"#);
        assert_eq!(blank.attribute("hw_serial"), None, "blank is absent");
    }
}
