//! The block thread: TAI-clocked write of the output flow and read of the input
//! flow, one block per period.
//!
//! Block n covers output samples [n·B, (n+1)·B). The thread sleeps until the TAI
//! time of sample (n+1)·B (`clock_nanosleep(CLOCK_TAI, TIMER_ABSTIME)`), writes
//! the block, and reads the input block [n·B − R, (n+1)·B − R), R = read offset.
//! MXL addresses a range by its head (end) index.
//! Readers are opened by the control thread and handed over through a channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mxl::{Error as MxlError, SamplesReader, SamplesWriter};

use crate::stats::Histogram;
use crate::tone;

pub const RATE: mxl::Rational = mxl::Rational {
    numerator: 48_000,
    denominator: 1,
};

pub enum Command {
    /// Read this flow from now on (activation accepted).
    Input(Box<SamplesReader>),
    /// Stop reading (`master_enable: false` or a new activation).
    NoInput,
}

pub struct Config {
    pub block: u64,
    pub read_offset: u64,
    pub read_timeout: Duration,
    pub verify: bool,
    pub rt_priority: Option<i32>,
    pub cpu: Option<usize>,
    /// What [`reserve_core`] did with the other threads, for the report.
    pub core: String,
}

/// The CPUs of `cpu`'s core other than itself (SMT siblings, from sysfs).
fn siblings(cpu: usize) -> Option<Vec<usize>> {
    let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
    let text = std::fs::read_to_string(path).ok()?;
    let mut out = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
            out.extend((a..=b).filter(|c| *c != cpu));
        }
    }
    Some(out)
}

/// The CPUs the other threads may use, kept off `cpu` and its SMT siblings when
/// there are CPUs on another core (else off `cpu` only), as the mixer does.
fn other_cpus(cpus: &[usize], cpu: usize, siblings: Option<&[usize]>) -> Vec<usize> {
    let core: Vec<usize> = std::iter::once(cpu)
        .chain(siblings.unwrap_or_default().iter().copied())
        .collect();
    let rest: Vec<usize> = cpus.iter().copied().filter(|c| !core.contains(c)).collect();
    if rest.is_empty() {
        cpus.iter().copied().filter(|c| *c != cpu).collect()
    } else {
        rest
    }
}

/// Keep every thread started after this call off the block thread's core (its
/// CPU and that CPU's SMT siblings). Call before any thread starts. Returns a
/// note for the report.
pub fn reserve_core(cpu: usize) -> String {
    // SAFETY: plain libc calls on zeroed CPU sets for the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return "CPUs unknown".into();
        }
        let cpus: Vec<usize> = (0..libc::CPU_SETSIZE as usize)
            .filter(|c| libc::CPU_ISSET(*c, &set))
            .collect();
        let sib = siblings(cpu);
        let rest = other_cpus(&cpus, cpu, sib.as_deref());
        if rest.is_empty() {
            return "no other CPU for the other threads".into();
        }
        let mut narrowed: libc::cpu_set_t = std::mem::zeroed();
        for c in &rest {
            libc::CPU_SET(*c, &mut narrowed);
        }
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &narrowed) != 0 {
            return format!(
                "keeping the other threads off CPU {cpu}'s core failed ({})",
                std::io::Error::last_os_error()
            );
        }
        let list = rest
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let outside: Vec<String> = sib
            .unwrap_or_default()
            .iter()
            .filter(|s| !cpus.contains(s))
            .map(|s| s.to_string())
            .collect();
        if outside.is_empty() {
            format!("other threads on CPU {list}")
        } else {
            format!(
                "other threads on CPU {list}; the core is shared with CPU {}, outside this process",
                outside.join(",")
            )
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Report {
    pub blocks: u64,
    pub written_samples: u64,
    pub read_samples: u64,
    pub too_early_blocks: u64,
    pub too_late_blocks: u64,
    pub timeouts: u64,
    pub other_read_errors: u64,
    pub write_errors: u64,
    pub mismatched_samples: u64,
    pub overruns: u64,
}

pub struct Shared {
    pub report: Mutex<Report>,
    pub wake_jitter: Mutex<Histogram>,
    pub block_work: Mutex<Histogram>,
    pub arrival_lag: Mutex<Histogram>,
    pub stop: AtomicBool,
    pub sched: Mutex<String>,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            report: Mutex::new(Report::default()),
            wake_jitter: Mutex::new(Histogram::new()),
            block_work: Mutex::new(Histogram::new()),
            arrival_lag: Mutex::new(Histogram::new()),
            stop: AtomicBool::new(false),
            sched: Mutex::new(String::new()),
        })
    }
}

fn tai_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid timespec, CLOCK_TAI exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_TAI, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn sleep_until_tai(ns: u64) {
    let ts = libc::timespec {
        tv_sec: (ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (ns % 1_000_000_000) as libc::c_long,
    };
    // SAFETY: valid timespec; EINTR just ends the sleep early, the caller re-checks.
    unsafe {
        libc::clock_nanosleep(
            libc::CLOCK_TAI,
            libc::TIMER_ABSTIME,
            &ts,
            std::ptr::null_mut(),
        );
    }
}

/// Time of sample `index` on the TAI timeline (ns since the TAI epoch).
fn index_to_ns(index: u64) -> u64 {
    (u128::from(index) * 1_000_000_000 / 48_000) as u64
}

fn set_up_thread(config: &Config, shared: &Shared) {
    let mut notes = Vec::new();
    // Flush-to-zero and denormals-are-zero, as the DSP thread will.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        #[allow(deprecated)]
        std::arch::x86_64::_mm_setcsr(std::arch::x86_64::_mm_getcsr() | 0x8040);
        notes.push("FTZ/DAZ".to_string());
    }
    // The default 50 µs timer slack of a normal thread delays every wake-up.
    // SAFETY: plain prctl on the current thread.
    if unsafe { libc::prctl(libc::PR_SET_TIMERSLACK, 1u64, 0u64, 0u64, 0u64) } == 0 {
        notes.push("timer slack 1 ns".to_string());
    }
    if !config.core.is_empty() {
        notes.push(config.core.clone());
    }
    if let Some(cpu) = config.cpu {
        // SAFETY: plain libc calls on the current thread.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0 {
                notes.push(format!("pinned to CPU {cpu}"));
            } else {
                notes.push(format!("pinning to CPU {cpu} failed"));
            }
        }
    }
    if let Some(priority) = config.rt_priority {
        let param = libc::sched_param {
            sched_priority: priority.min(49),
        };
        // SAFETY: plain libc call on the current thread.
        let rc =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
        notes.push(if rc == 0 {
            format!("SCHED_FIFO {}", priority.min(49))
        } else {
            format!(
                "SCHED_FIFO {} refused (rc {rc}), SCHED_OTHER",
                priority.min(49)
            )
        });
    } else {
        notes.push("SCHED_OTHER".to_string());
    }
    *shared.sched.lock().unwrap() = notes.join(", ");
}

pub fn run(
    config: Config,
    writer: SamplesWriter,
    channels: usize,
    commands: Receiver<Command>,
    shared: Arc<Shared>,
    done: Sender<()>,
) {
    set_up_thread(&config, &shared);
    let block = config.block;
    let mut input: Option<Box<SamplesReader>> = None;
    let mut n = tai_ns() as u128 * 48_000 / 1_000_000_000 / u128::from(block) + 1;
    let mut report = Report::default();
    let mut jitter = Histogram::new();
    let mut work = Histogram::new();
    let mut lag = Histogram::new();
    let mut since_publish = 0u32;
    while !shared.stop.load(Ordering::Relaxed) {
        let start = (n as u64) * block;
        let deadline = index_to_ns(start + block);
        sleep_until_tai(deadline);
        let woke = tai_ns();
        if woke < deadline {
            continue;
        }
        jitter.record(woke - deadline);
        let late_by = woke - deadline;
        if late_by > index_to_ns(block) {
            report.overruns += 1;
        }

        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Input(reader) => input = Some(reader),
                Command::NoInput => input = None,
            }
        }

        // MXL sample indices are head indices: (index, count) is [index − count, index).
        match writer.open_samples(start + block, block as usize) {
            Ok(mut access) => {
                for c in 0..channels.min(access.channels()) {
                    if let Ok((a, b)) = access.channel_data_mut(c) {
                        let chunks = a.chunks_exact_mut(4).chain(b.chunks_exact_mut(4));
                        for (i, chunk) in (start..).zip(chunks) {
                            chunk.copy_from_slice(&tone::sample(c, i).to_le_bytes());
                        }
                    }
                }
                if access.commit().is_ok() {
                    report.written_samples += block;
                } else {
                    report.write_errors += 1;
                }
            }
            Err(_) => report.write_errors += 1,
        }

        if let Some(reader) = &input {
            let from = start - config.read_offset;
            if let Ok(runtime) = reader.get_runtime_info() {
                let now_index = (u128::from(tai_ns()) * 48_000 / 1_000_000_000) as u64;
                lag.record(index_to_ns(now_index.saturating_sub(runtime.headIndex)));
            }
            match reader.get_samples(from + block, block as usize, config.read_timeout) {
                Ok(data) => {
                    report.read_samples += block;
                    if config.verify {
                        for c in 0..data.num_of_channels() {
                            if let Ok((a, b)) = data.channel_data(c) {
                                let chunks = a.chunks_exact(4).chain(b.chunks_exact(4));
                                for (i, chunk) in (from..).zip(chunks) {
                                    let v = f32::from_le_bytes([
                                        chunk[0], chunk[1], chunk[2], chunk[3],
                                    ]);
                                    if v != tone::sample(c, i) {
                                        report.mismatched_samples += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(MxlError::OutOfRangeTooEarly) => report.too_early_blocks += 1,
                Err(MxlError::OutOfRangeTooLate) => report.too_late_blocks += 1,
                Err(MxlError::Timeout) => report.timeouts += 1,
                Err(_) => report.other_read_errors += 1,
            }
        }
        work.record(tai_ns() - woke);
        report.blocks += 1;
        n += 1;

        since_publish += 1;
        if since_publish >= 500 {
            since_publish = 0;
            // try_lock: the block thread never waits for the control side.
            if let Ok(mut shared_report) = shared.report.try_lock() {
                *shared_report = report;
            }
        }
    }
    *shared.report.lock().unwrap() = report;
    *shared.wake_jitter.lock().unwrap() = jitter;
    *shared.block_work.lock().unwrap() = work;
    *shared.arrival_lag.lock().unwrap() = lag;
    drop(writer);
    let _ = done.send(());
}

#[cfg(test)]
mod tests {
    use super::other_cpus;

    #[test]
    fn the_other_threads_keep_off_the_block_threads_core() {
        assert_eq!(other_cpus(&[3, 4, 11, 12], 11, Some(&[3])), [4, 12]);
        assert_eq!(other_cpus(&[3, 11], 11, Some(&[3])), [3]);
        assert_eq!(other_cpus(&[20, 21, 23], 23, Some(&[47])), [20, 21]);
        assert_eq!(other_cpus(&[0, 1], 1, None), [0]);
        assert!(other_cpus(&[5], 5, Some(&[])).is_empty());
    }
}
