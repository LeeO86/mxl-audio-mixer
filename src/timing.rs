//! TAI time, the block clock, and the thread set-up rules of §2 and §18.

pub const NANOS: u64 = 1_000_000_000;

/// Nanoseconds since the TAI epoch (`CLOCK_TAI`, the clock MXL uses).
pub fn tai_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid timespec; CLOCK_TAI exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_TAI, &mut ts) };
    ts.tv_sec as u64 * NANOS + ts.tv_nsec as u64
}

/// Sleep until the TAI time `ns` (absolute). Returns early on a signal.
pub fn sleep_until_tai(ns: u64) {
    let ts = libc::timespec {
        tv_sec: (ns / NANOS) as libc::time_t,
        tv_nsec: (ns % NANOS) as libc::c_long,
    };
    // SAFETY: valid timespec.
    unsafe {
        libc::clock_nanosleep(
            libc::CLOCK_TAI,
            libc::TIMER_ABSTIME,
            &ts,
            std::ptr::null_mut(),
        )
    };
}

/// The 48 kHz sample index at TAI time `ns`.
pub fn index_at(ns: u64) -> u64 {
    (u128::from(ns) * 48_000 / u128::from(NANOS)) as u64
}

/// The TAI time of sample index `index`.
pub fn time_of(index: u64) -> u64 {
    (u128::from(index) * u128::from(NANOS)).div_ceil(48_000) as u64
}

/// TAI−UTC since 2017.
pub const TAI_UTC_OFFSET_S: i64 = 37;

/// §4.4: the kernel's TAI offset must be the current TAI−UTC offset; otherwise
/// every MXL grain is 37 s off. Returns the measured offset and whether it is right.
pub fn check_tai_offset() -> (i64, bool) {
    let offset = mxl_nmos::clock::tai_utc_offset_secs();
    (offset, offset == TAI_UTC_OFFSET_S)
}

/// The container's CPU limit in cores (`/sys/fs/cgroup/cpu.max`), if any.
pub fn cgroup_cpu_limit() -> Option<f64> {
    let text = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let mut parts = text.split_whitespace();
    let quota = parts.next()?;
    let period: f64 = parts.next()?.parse().ok()?;
    if quota == "max" || period <= 0.0 {
        return None;
    }
    Some(quota.parse::<f64>().ok()? / period)
}

/// CPUs in this process's affinity mask.
pub fn affinity_cpus() -> Vec<usize> {
    // SAFETY: plain libc calls with a zeroed set.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return Vec::new();
        }
        (0..libc::CPU_SETSIZE as usize)
            .filter(|cpu| libc::CPU_ISSET(*cpu, &set))
            .collect()
    }
}

/// Worker threads for the non-real-time pools: the cgroup limit (or the affinity
/// mask) minus one core for the DSP thread, at least one (§2).
pub fn worker_threads() -> usize {
    let cpus = affinity_cpus().len().max(1);
    let limit = cgroup_cpu_limit()
        .map(|c| c.floor() as usize)
        .unwrap_or(cpus)
        .min(cpus);
    limit.saturating_sub(1).max(1)
}

/// The CPU the DSP thread is pinned to: the last one of the affinity mask, so the
/// tokio workers (first CPUs) and the DSP thread rarely share one.
pub fn dsp_cpu() -> Option<usize> {
    let cpus = affinity_cpus();
    if cpus.len() > 1 {
        cpus.last().copied()
    } else {
        None
    }
}

/// Flush denormals to zero on the calling thread (decaying meters and filters
/// reach them, and they are slow). Returns what was set.
pub fn flush_denormals() -> Option<&'static str> {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: setting FTZ (bit 15) and DAZ (bit 6) in MXCSR only changes how
    // denormals are handled on this thread.
    unsafe {
        #[allow(deprecated)]
        std::arch::x86_64::_mm_setcsr(std::arch::x86_64::_mm_getcsr() | 0x8040);
        return Some("FTZ/DAZ");
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: setting FPCR.FZ only changes denormal handling on this thread.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr);
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr);
        return Some("FZ");
    }
    #[allow(unreachable_code)]
    None
}

/// Keep the calling thread off `cpu` (the DSP thread's), when the process may
/// run on other CPUs too.
pub fn avoid_cpu(cpu: usize) {
    let others: Vec<usize> = affinity_cpus().into_iter().filter(|c| *c != cpu).collect();
    if others.is_empty() {
        return;
    }
    // SAFETY: a CPU set built and applied to the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for c in others {
            libc::CPU_SET(c, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

/// Set up the calling thread as the DSP thread. Returns a description.
pub fn set_up_dsp_thread(rt_priority: Option<i32>, cpu: Option<usize>) -> String {
    let mut notes = Vec::new();
    if let Some(f) = flush_denormals() {
        notes.push(f.to_string());
    }
    // SAFETY: plain prctl/sched calls on the current thread.
    unsafe {
        if libc::prctl(libc::PR_SET_TIMERSLACK, 1u64, 0u64, 0u64, 0u64) == 0 {
            notes.push("timer slack 1 ns".to_string());
        }
        if let Some(cpu) = cpu {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0 {
                notes.push(format!("CPU {cpu}"));
            }
        }
        match rt_priority {
            Some(priority) => {
                let param = libc::sched_param {
                    sched_priority: priority.min(49),
                };
                let rc =
                    libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param);
                if rc == 0 {
                    notes.push(format!("SCHED_FIFO {}", priority.min(49)));
                } else {
                    notes.push(format!(
                        "SCHED_FIFO {} refused ({}), SCHED_OTHER",
                        priority.min(49),
                        std::io::Error::from_raw_os_error(rc)
                    ));
                }
            }
            None => notes.push("SCHED_OTHER".to_string()),
        }
    }
    notes.join(", ")
}

/// CPU time of this process in seconds.
pub fn process_cpu_seconds() -> f64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: valid rusage pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

/// Resident memory of this process in bytes.
pub fn process_resident_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<u64>().ok())
        })
        .map(|pages| pages * 4096)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_time_round_trip() {
        let t = 1_800_000_000 * NANOS + 123_456_789;
        let i = index_at(t);
        assert!(time_of(i) <= t && time_of(i + 1) > t);
        assert_eq!(index_at(time_of(i)), i);
        assert_eq!(time_of(48_000), NANOS);
    }

    #[test]
    fn workers_leave_a_core() {
        assert!(worker_threads() >= 1);
    }
}
