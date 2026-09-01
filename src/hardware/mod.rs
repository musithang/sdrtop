pub mod hackrf;
pub mod process;
pub mod rtlsdr;
pub mod soapy;
pub mod sysfs;
pub mod traits;

pub use hackrf::{board_rev_name, compute_bb_filter_bw};
pub use traits::{
    DeviceCapabilities, DeviceInfo, GainModel, RxContext, SampleFormat, SampleGeometry, SdrDevice,
};

use std::sync::Arc;

/// Which backend a [`DeviceListing`] / open request targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    HackRf,
    RtlSdr,
    Soapy,
}

/// One enumerated device, before it is opened. `index` is the per-backend index
/// the backend's `open(index)` expects. `label` is the human string shown in the
/// device selector.
#[derive(Clone, Debug)]
pub struct DeviceListing {
    pub kind: DeviceKind,
    pub index: usize,
    pub label: String,
    /// SoapySDR device arguments, `None` for the two native backends.
    ///
    /// Soapy identifies a device by key/value arguments, not by a position in a
    /// list, and an index into a list that may have been re-enumerated since is
    /// a race. The two native backends genuinely are index addressed, so they
    /// carry nothing here.
    pub args: Option<String>,
}

/// Every connected device across all compiled-in backends. Never fails: a
/// backend with no devices (or an enumeration error) simply contributes nothing.
pub fn list_all_devices() -> Vec<DeviceListing> {
    let mut out = Vec::new();
    out.extend(hackrf::list());
    out.extend(rtlsdr::list());
    out
}

/// Opens the device a listing points at, as a trait object.
pub fn open_device(listing: &DeviceListing) -> anyhow::Result<Arc<dyn SdrDevice>> {
    match listing.kind {
        DeviceKind::HackRf => Ok(Arc::new(hackrf::HackRfDevice::open(listing.index)?)),
        DeviceKind::RtlSdr => Ok(Arc::new(rtlsdr::RtlDevice::open(listing.index)?)),
        DeviceKind::Soapy => {
            let Some(args) = listing.args.as_deref() else {
                anyhow::bail!("a SoapySDR listing with no device arguments cannot be opened");
            };
            Ok(Arc::new(soapy::device::SoapyDevice::open(args)?))
        }
    }
}
