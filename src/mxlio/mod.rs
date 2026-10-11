//! MXL I/O: the library, domains (G2), readers and writers.

pub mod domain;
pub mod writer;

use std::path::Path;
use std::sync::Arc;

use uuid::Uuid;

pub use mxl::{MxlApi, MxlInstance, SamplesReader};

/// libmxl, found through the dynamic linker (the image installs it in the linker
/// path; locally `LD_LIBRARY_PATH=/opt/mxl/lib`).
pub const LIBRARY: &str = "libmxl.so.1";

pub type Api = Arc<MxlApi>;

pub fn load() -> Result<Api, String> {
    mxl::load_api(LIBRARY).map_err(|e| format!("cannot load {LIBRARY}: {e}"))
}

/// An MXL flow's directory in a domain.
pub fn flow_dir(domain: &Path, flow: Uuid) -> std::path::PathBuf {
    domain.join(format!("{flow}.mxl-flow"))
}

/// The inode of a flow's data file: it changes when the flow is deleted and
/// re-created by a restarted writer (§4.2).
pub fn flow_inode(domain: &Path, flow: Uuid) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(flow_dir(domain, flow).join("data"))
        .ok()
        .map(|m| m.ino())
}

/// Open a continuous-flow reader.
pub fn open_reader(instance: &MxlInstance, flow: Uuid) -> Result<(SamplesReader, u32), mxl::Error> {
    let reader = instance
        .create_flow_reader(&flow.to_string())?
        .to_samples_reader()?;
    let channels = reader
        .get_config_info()
        .and_then(|info| info.continuous().map(|c| c.channelCount))
        .unwrap_or(0);
    Ok((reader, channels))
}
