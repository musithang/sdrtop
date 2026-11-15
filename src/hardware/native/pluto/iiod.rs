// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 MusiThang <viktor.laszlo92@protonmail.com>

//! A client for `iiod`'s text protocol, over TCP.
//!
//! Every PlutoSDR-family radio runs `iiod`, the server libiio's clients talk
//! to. sdrtop speaks its text protocol itself rather than loading libiio: one
//! less library to install, and every byte on the wire is something the tests
//! can play back. The protocol is the one libiio 0.25's `iiod` serves (the
//! PlutoSky R2's) and 1.x's still starts every session in; this client knows
//! commands and replies and nothing about any radio.
//!
//! **The framing, as the server writes it** (libiio `iiod/ops.c`, checked on
//! the radio): a command is one line; every reply starts with a decimal
//! integer line, negative for `-errno`, except `VERSION`, which is one line of
//! text. `PRINT` answers the length, then that many bytes of XML, then a line
//! end.
//!
//! **A dead radio is a refusal, never a hang.** Every connection carries
//! connect, read and write timeouts. A radio pulled off the cable does not
//! close the socket: the network interface just disappears, and silence is
//! all the other end ever sends. The timeouts are what turn that into an
//! answer.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The port `iiod` listens on.
pub const PORT: u16 = 30431;

/// The longest context description accepted. The R2's is 32 KB; a length far
/// beyond any radio's is a stream out of step, not a description.
const MAX_XML_BYTES: usize = 16 << 20;

/// How long each kind of wait may last.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    pub io: Duration,
}

/// Why an exchange with the server failed.
#[derive(Debug)]
pub enum Error {
    /// The socket: refused, timed out, reset.
    Io(io::Error),
    /// The server answered `-errno`.
    Server(i64),
    /// The reply did not have the shape the protocol gives it.
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) if is_timeout(e) => write!(f, "no answer in time"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Server(code) => write!(f, "the server answered {code}"),
            Error::Protocol(what) => write!(f, "an answer out of step with the protocol: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// The server's version, as `VERSION` gives it: `0.25.v0.25`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    /// The git tag or hash after the numbers, trimmed of its padding.
    pub tag: String,
}

impl Version {
    /// `"0.25"`, the form the checked list is written in.
    pub fn short(&self) -> String {
        format!("{}.{}", self.major, self.minor)
    }

    /// Reads `VERSION`'s line. The server pads the tag to seven characters.
    pub fn parse(line: &str) -> Option<Version> {
        let line = line.trim();
        let mut parts = line.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let tag = parts.next().unwrap_or("").trim().to_string();
        Some(Version { major, minor, tag })
    }
}

/// What an attribute belongs to, as the protocol names it.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Device(&'a str),
    Input(&'a str, &'a str),
    Output(&'a str, &'a str),
    Debug(&'a str),
}

impl fmt::Display for Target<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Device(d) => write!(f, "{d}"),
            Target::Input(d, c) => write!(f, "{d} INPUT {c}"),
            Target::Output(d, c) => write!(f, "{d} OUTPUT {c}"),
            Target::Debug(d) => write!(f, "{d} DEBUG"),
        }
    }
}

/// One session with an `iiod` server.
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

impl Client {
    /// Connects to `host` (an address or a name, with or without a port; the
    /// port defaults to [`PORT`]).
    pub fn connect(host: &str, t: Timeouts) -> Result<Client, Error> {
        let addr = resolve(host)?;
        let stream = TcpStream::connect_timeout(&addr, t.connect)?;
        stream.set_read_timeout(Some(t.io))?;
        stream.set_write_timeout(Some(t.io))?;
        // Commands are a few bytes each, sent one at a time, each waiting for
        // its reply: Nagle would only add a delay to every one.
        stream.set_nodelay(true)?;
        let writer = stream.try_clone()?;
        Ok(Client {
            reader: BufReader::new(stream),
            writer,
        })
    }

    fn send(&mut self, line: &str) -> Result<(), Error> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\r\n")?;
        Ok(())
    }

    fn read_line(&mut self) -> Result<String, Error> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the server closed the connection",
            )));
        }
        Ok(line)
    }

    /// The integer every reply starts with; blank lines before it are
    /// skipped, as libiio's own client does. Negative is the server's
    /// `-errno`.
    fn read_integer(&mut self) -> Result<i64, Error> {
        loop {
            let line = self.read_line()?;
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            let value: i64 = t
                .parse()
                .map_err(|_| Error::Protocol(format!("expected a number, got {t:?}")))?;
            return if value < 0 {
                Err(Error::Server(value))
            } else {
                Ok(value)
            };
        }
    }

    /// `VERSION`.
    pub fn version(&mut self) -> Result<Version, Error> {
        self.send("VERSION")?;
        let line = self.read_line()?;
        Version::parse(&line).ok_or_else(|| Error::Protocol(format!("a version of {line:?}")))
    }

    /// `PRINT`: the context description.
    pub fn print(&mut self) -> Result<String, Error> {
        self.send("PRINT")?;
        let len = self.read_integer()? as usize;
        if len > MAX_XML_BYTES {
            return Err(Error::Protocol(format!("a description {len} bytes long")));
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf)?;
        let mut end = [0u8; 1];
        self.reader.read_exact(&mut end)?;
        String::from_utf8(buf)
            .map_err(|_| Error::Protocol("a description that is not UTF-8".into()))
    }

    /// `READ`: one attribute's value, the trailing NUL the server counts in
    /// its length removed.
    pub fn read(&mut self, target: Target<'_>, attr: &str) -> Result<String, Error> {
        self.send(&format!("READ {target} {attr}"))?;
        let len = self.read_integer()? as usize;
        if len > MAX_XML_BYTES {
            return Err(Error::Protocol(format!("a value {len} bytes long")));
        }
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf)?;
        let mut end = [0u8; 1];
        self.reader.read_exact(&mut end)?;
        if buf.last() == Some(&0) {
            buf.pop();
        }
        String::from_utf8(buf).map_err(|_| Error::Protocol("a value that is not UTF-8".into()))
    }

    /// `WRITE`: the line, then exactly the value's bytes; the server answers
    /// how many it wrote.
    pub fn write(&mut self, target: Target<'_>, attr: &str, value: &str) -> Result<(), Error> {
        self.send(&format!("WRITE {target} {attr} {}", value.len()))?;
        self.writer.write_all(value.as_bytes())?;
        self.read_integer()?;
        Ok(())
    }

    /// One register of a device's core, read through its `direct_reg_access`
    /// debug attribute: writing one number selects a register (writing two
    /// would write it), reading returns the selected register's value.
    pub fn register(&mut self, device: &str, address: u32) -> Result<u32, Error> {
        self.write(
            Target::Debug(device),
            "direct_reg_access",
            &format!("0x{address:x}"),
        )?;
        let v = self.read(Target::Debug(device), "direct_reg_access")?;
        let hex = v.trim().trim_start_matches("0x").trim_start_matches("0X");
        u32::from_str_radix(hex, 16)
            .map_err(|_| Error::Protocol(format!("a register value of {v:?}")))
    }

    /// `EXIT`: the server ends the session without a reply. Best effort, since
    /// the socket closes on drop either way.
    pub fn exit(mut self) {
        let _ = self.send("EXIT");
    }
}

fn resolve(host: &str) -> Result<SocketAddr, Error> {
    let with_port = if host.parse::<SocketAddr>().is_ok()
        || host
            .rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        host.to_string()
    } else {
        format!("{host}:{PORT}")
    };
    with_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{host} resolves to nothing")))
}

/// A stand-in `iiod` for the tests: a thread on `127.0.0.1` answering the
/// commands the way the R2's server did, from what was recorded on the bench,
/// and keeping what was written so a test can read the radio's state after.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::{HashMap, HashSet};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// The radio as the stand-in holds it.
    #[derive(Clone, Default)]
    pub struct Radio {
        /// `VERSION`'s line, as the server writes it.
        pub version: String,
        /// `PRINT`'s document.
        pub xml: String,
        /// Accept the connection and never answer: a radio gone silent.
        pub silent: bool,
        /// Attribute values by `"<target> <attr>"`, as the commands spell them.
        pub attrs: HashMap<String, String>,
        /// Core registers by `(device, address)`.
        pub regs: HashMap<(String, u32), u32>,
        /// Writes the radio accepts but does not keep: the value read back
        /// stays as it was. A radio that will not do what it is told.
        pub ignores: HashSet<String>,
        /// Every write, in order, as `"<target> <attr>=<value>"`.
        pub written: Vec<String>,
        /// The register selected through each device's `direct_reg_access`.
        pub selected: HashMap<String, u32>,
    }

    /// Starts the stand-in; it serves one connection. Returns its address and
    /// the radio, to look at once the session is over.
    pub fn serve(radio: Radio) -> (String, Arc<Mutex<Radio>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port on 127.0.0.1");
        let addr = listener.local_addr().unwrap().to_string();
        let shared = Arc::new(Mutex::new(radio));
        let state = Arc::clone(&shared);
        thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut w = stream.try_clone().unwrap();
            let mut r = BufReader::new(stream);
            if state.lock().unwrap().silent {
                let mut sink = String::new();
                while matches!(r.read_line(&mut sink), Ok(n) if n > 0) {}
                return;
            }
            let mut line = String::new();
            while matches!(r.read_line(&mut line), Ok(n) if n > 0) {
                let reply = answer(&state, line.trim(), &mut r);
                let Some(reply) = reply else { return };
                if w.write_all(&reply).is_err() {
                    return;
                }
                line.clear();
            }
        });
        (addr, shared)
    }

    fn value(v: &str) -> Vec<u8> {
        // The server counts the value's trailing NUL in its length.
        let mut out = format!("{}\n", v.len() + 1).into_bytes();
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\0\n");
        out
    }

    fn answer(
        state: &Arc<Mutex<Radio>>,
        line: &str,
        r: &mut BufReader<std::net::TcpStream>,
    ) -> Option<Vec<u8>> {
        let mut radio = state.lock().unwrap();
        let words: Vec<&str> = line.split(' ').collect();
        Some(match words.as_slice() {
            ["VERSION"] => radio.version.clone().into_bytes(),
            ["PRINT"] => {
                let mut out = format!("{}\n", radio.xml.len()).into_bytes();
                out.extend_from_slice(radio.xml.as_bytes());
                out.push(b'\n');
                out
            }
            ["EXIT"] => return None,
            ["READ", rest @ ..] => {
                let key = rest.join(" ");
                if let [dev, "DEBUG", "direct_reg_access"] = rest {
                    let at = radio.selected.get(*dev).copied().unwrap_or(0);
                    let v = radio.regs.get(&(dev.to_string(), at)).copied().unwrap_or(0);
                    return Some(value(&format!("0x{v:X}")));
                }
                match radio.attrs.get(&key) {
                    Some(v) => value(v),
                    None => b"-2\n".to_vec(),
                }
            }
            ["WRITE", rest @ .., len] => {
                let n: usize = len.parse().ok()?;
                let mut buf = vec![0u8; n];
                r.read_exact(&mut buf).ok()?;
                let v = String::from_utf8(buf).ok()?;
                let key = rest.join(" ");
                radio.written.push(format!("{key}={v}"));
                if let [dev, "DEBUG", "direct_reg_access"] = rest {
                    let hex = v.trim().trim_start_matches("0x");
                    let at = u32::from_str_radix(hex, 16).ok()?;
                    radio.selected.insert(dev.to_string(), at);
                } else if !radio.attrs.contains_key(&key) {
                    return Some(b"-2\n".to_vec());
                } else if !radio.ignores.contains(&key) {
                    radio.attrs.insert(key, v);
                }
                format!("{n}\n").into_bytes()
            }
            // What the server answers a line it cannot parse.
            _ => b"-22\n".to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick() -> Timeouts {
        Timeouts {
            connect: Duration::from_millis(500),
            io: Duration::from_millis(300),
        }
    }

    fn r2_script() -> fake::Radio {
        fake::Radio {
            // Exactly what the R2's server wrote on the bench.
            version: "0.25.v0.25  \n".into(),
            xml: include_str!("../fixtures/pluto-r2-context.xml").into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_version_line_reads_as_the_r2_wrote_it() {
        let v = Version::parse("0.25.v0.25  \n").unwrap();
        assert_eq!((v.major, v.minor, v.tag.as_str()), (0, 25, "v0.25"));
        assert_eq!(v.short(), "0.25");
        assert_eq!(Version::parse("1.0.abcdef0").unwrap().short(), "1.0");
        assert!(Version::parse("ERROR").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn a_session_reads_the_version_and_the_description() {
        let (addr, _) = fake::serve(r2_script());
        let mut c = Client::connect(&addr, quick()).unwrap();
        assert_eq!(c.version().unwrap().short(), "0.25");
        let xml = c.print().unwrap();
        assert_eq!(xml.len(), 32167, "the R2's description, byte for byte");
        assert!(xml.ends_with("</context>"));
        c.exit();
    }

    /// The server answers `-22` to a line it cannot parse; the client says
    /// so as the server's answer rather than as a broken stream.
    #[test]
    fn a_negative_reply_is_the_servers_answer() {
        let (addr, _) = fake::serve(r2_script());
        let mut c = Client::connect(&addr, quick()).unwrap();
        c.send("FROBNICATE").unwrap();
        match c.read_integer() {
            Err(Error::Server(-22)) => {}
            other => panic!("expected the server's -22, got {other:?}"),
        }
    }

    /// A radio that accepts and then says nothing (pulled off its cable, the
    /// server stuck) is an answer within the timeout, not a hang.
    #[test]
    fn silence_is_a_refusal_within_the_timeout() {
        let (addr, _) = fake::serve(fake::Radio {
            silent: true,
            ..r2_script()
        });
        let mut c = Client::connect(&addr, quick()).unwrap();
        let started = std::time::Instant::now();
        let err = c.version().unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(err.to_string(), "no answer in time");
    }

    /// Nobody listening: refused at once.
    #[test]
    fn nobody_listening_is_refused() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(Client::connect(&format!("127.0.0.1:{port}"), quick()).is_err());
    }

    #[test]
    fn a_bare_address_gets_the_iiod_port() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), PORT);
        assert_eq!(resolve("127.0.0.1:5000").unwrap().port(), 5000);
    }
}
