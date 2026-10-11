//! Continuous-flow writers through `mxl-sys`.
//!
//! The safe `MxlInstance::create_flow_writer` at the pinned revision frees the
//! options string before libmxl reads it, so the commit and sync batch hints
//! (`MIXER_OUTPUT_COMMIT_SAMPLES`) could not be set. This wrapper owns its own
//! instance and passes the options while they are alive. Back to the safe API once
//! a v1.1 release has the upstream fix (aed4cac6).

use std::ffi::CString;
use std::path::Path;

use super::Api;

fn status(code: mxl_sys::Status, what: &str) -> Result<(), String> {
    if code == mxl_sys::MXL_STATUS_OK {
        Ok(())
    } else {
        Err(format!("{what}: MXL status {code}"))
    }
}

/// An MXL instance for writers in one domain.
pub struct WriterInstance {
    api: Api,
    instance: mxl_sys::Instance,
}

// The MXL API is thread-safe at the instance level.
unsafe impl Send for WriterInstance {}
unsafe impl Sync for WriterInstance {}

impl WriterInstance {
    pub fn new(api: Api, domain: &Path) -> Result<Self, String> {
        let domain = CString::new(domain.display().to_string()).map_err(|e| e.to_string())?;
        let options = CString::new("").map_err(|e| e.to_string())?;
        // SAFETY: both strings live until the call returns.
        let instance = unsafe { api.create_instance(domain.as_ptr(), options.as_ptr()) };
        if instance.is_null() {
            return Err("MXL instance for the output domain could not be created".into());
        }
        Ok(Self { api, instance })
    }

    /// Create (or open) a continuous flow. `commit_samples` becomes both the commit
    /// and the sync batch hint.
    pub fn create_samples_writer(
        &self,
        flow_def: &str,
        commit_samples: u32,
    ) -> Result<SamplesWriter<'_>, String> {
        let def = CString::new(flow_def).map_err(|e| e.to_string())?;
        let options = CString::new(
            serde_json::json!({"maxCommitBatchSizeHint": commit_samples, "maxSyncBatchSizeHint": commit_samples}).to_string(),
        )
        .map_err(|e| e.to_string())?;
        let mut writer: mxl_sys::FlowWriter = std::ptr::null_mut();
        let mut info = std::mem::MaybeUninit::<mxl_sys::FlowConfigInfo>::zeroed();
        let mut created = false;
        // SAFETY: the strings and out-pointers are valid for the duration of the call.
        let code = unsafe {
            self.api.create_flow_writer(
                self.instance,
                def.as_ptr(),
                options.as_ptr(),
                &mut writer,
                info.as_mut_ptr(),
                &mut created,
            )
        };
        status(code, "create flow writer")?;
        if writer.is_null() {
            return Err("create flow writer returned no writer".into());
        }
        // SAFETY: initialised by libmxl on success.
        let info = unsafe { info.assume_init() };
        // SAFETY: the union holds the continuous part for audio flows.
        let (channels, buffer) = unsafe {
            (
                info.__bindgen_anon_1.continuous.channelCount,
                info.__bindgen_anon_1.continuous.bufferLength,
            )
        };
        Ok(SamplesWriter {
            owner: self,
            writer,
            channels: channels as usize,
            buffer_length: buffer,
            commit_hint: info.common.maxCommitBatchSizeHint,
            created,
        })
    }
}

impl Drop for WriterInstance {
    fn drop(&mut self) {
        // SAFETY: the instance was created by this wrapper and all writers borrow it.
        unsafe { self.api.destroy_instance(self.instance) };
    }
}

/// A continuous-flow writer. Not thread-safe; owned by one thread at a time.
pub struct SamplesWriter<'a> {
    owner: &'a WriterInstance,
    writer: mxl_sys::FlowWriter,
    pub channels: usize,
    pub buffer_length: u32,
    pub commit_hint: u32,
    pub created: bool,
}

unsafe impl Send for SamplesWriter<'_> {}

impl SamplesWriter<'_> {
    /// Write `count` samples per channel ending before head index `head`
    /// (`[head − count, head)`). `fill(channel, offset, samples)` fills the samples
    /// starting at `offset` within the range (a range that wraps the ring comes in
    /// two pieces).
    pub fn write(
        &mut self,
        head: u64,
        count: usize,
        mut fill: impl FnMut(usize, usize, &mut [f32]),
    ) -> Result<(), String> {
        let mut slice = std::mem::MaybeUninit::<mxl_sys::MutableWrappedMultiBufferSlice>::zeroed();
        // SAFETY: valid writer and out-pointer.
        let code = unsafe {
            self.owner
                .api
                .flow_writer_open_samples(self.writer, head, count, slice.as_mut_ptr())
        };
        status(code, "open samples")?;
        // SAFETY: initialised on success.
        let slice = unsafe { slice.assume_init() };
        for c in 0..slice.count.min(self.channels) {
            let mut done = 0;
            for fragment in &slice.base.fragments {
                let n = fragment.size / 4;
                if n == 0 {
                    continue;
                }
                // SAFETY: libmxl hands out `size` bytes per channel at `pointer + stride * c`,
                // aligned for f32 samples.
                let samples = unsafe {
                    std::slice::from_raw_parts_mut(
                        (fragment.pointer as *mut u8)
                            .add(slice.stride * c)
                            .cast::<f32>(),
                        n,
                    )
                };
                fill(c, done, samples);
                done += n;
            }
            debug_assert_eq!(done, count);
        }
        // SAFETY: valid writer with an open range.
        status(
            unsafe { self.owner.api.flow_writer_commit_samples(self.writer) },
            "commit samples",
        )
    }
}

impl Drop for SamplesWriter<'_> {
    fn drop(&mut self) {
        // SAFETY: the writer belongs to this instance. Releasing the last writer
        // deletes the flow (§4.2), so writers are only dropped on unpublish or stop.
        unsafe {
            self.owner
                .api
                .release_flow_writer(self.owner.instance, self.writer)
        };
    }
}
