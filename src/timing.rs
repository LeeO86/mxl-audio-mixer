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
    let cpus = process_cpus().len().max(1);
    let limit = cgroup_cpu_limit()
        .map(|c| c.floor() as usize)
        .unwrap_or(cpus)
        .min(cpus);
    limit.saturating_sub(1).max(1)
}

/// The process's CPUs as at the first call (before [`reserve_dsp_core`]
/// narrows the threads started after it).
fn process_cpus() -> &'static [usize] {
    static CPUS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    CPUS.get_or_init(affinity_cpus)
}

/// The CPU the DSP thread is pinned to: the last one of the process's CPUs.
pub fn dsp_cpu() -> Option<usize> {
    let cpus = process_cpus();
    if cpus.len() > 1 {
        cpus.last().copied()
    } else {
        None
    }
}

/// A CPU list as in sysfs ("3,27", "0-1").
fn parse_cpu_list(s: &str) -> Vec<usize> {
    s.trim()
        .split(',')
        .filter(|p| !p.is_empty())
        .flat_map(|p| match p.split_once('-') {
            Some((a, b)) => match (a.parse::<usize>(), b.parse::<usize>()) {
                (Ok(a), Ok(b)) if a <= b => (a..=b).collect(),
                _ => Vec::new(),
            },
            None => p.parse().ok().into_iter().collect(),
        })
        .collect()
}

/// The other hardware threads of `cpu`'s core (SMT siblings); `None` when sysfs
/// does not say.
pub fn smt_siblings(cpu: usize) -> Option<Vec<usize>> {
    let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
    let list = std::fs::read_to_string(path).ok()?;
    Some(
        parse_cpu_list(&list)
            .into_iter()
            .filter(|c| *c != cpu)
            .collect(),
    )
}

/// The CPUs the other threads may use and what that means for the DSP thread's
/// core, from the process's CPUs, the DSP CPU and its siblings.
fn plan_cpus(cpus: &[usize], dsp: usize, siblings: Option<&[usize]>) -> (Vec<usize>, String) {
    let Some(siblings) = siblings else {
        let rest = cpus.iter().copied().filter(|c| *c != dsp).collect();
        return (rest, "its core's other CPUs unknown".into());
    };
    let core: Vec<usize> = std::iter::once(dsp)
        .chain(siblings.iter().copied())
        .collect();
    let rest: Vec<usize> = cpus.iter().copied().filter(|c| !core.contains(c)).collect();
    let outside: Vec<String> = siblings
        .iter()
        .filter(|s| !cpus.contains(s))
        .map(|s| s.to_string())
        .collect();
    let note = if siblings.is_empty() {
        "a core of its own (no SMT)".to_string()
    } else if outside.is_empty() && !rest.is_empty() {
        "a core of its own (its other CPUs kept free)".to_string()
    } else if outside.is_empty() {
        format!(
            "its core shared with the mixer's other threads (CPU {}): no other core",
            join(siblings)
        )
    } else {
        format!(
            "its core shared with CPU {}, outside this process: another workload may slow it",
            outside.join(",")
        )
    };
    if rest.is_empty() {
        return (cpus.iter().copied().filter(|c| *c != dsp).collect(), note);
    }
    (rest, note)
}

/// What the DSP thread's core is like (for its scheduling notes).
fn core_note(dsp: usize) -> String {
    plan_cpus(process_cpus(), dsp, smt_siblings(dsp).as_deref()).1
}

fn join(cpus: &[usize]) -> String {
    cpus.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Keep every thread started after this (the runtime's workers, the meters,
/// the preview, the control side) off the DSP thread's core: its CPU and that
/// CPU's SMT siblings, when the process has CPUs on other cores; else off the
/// DSP CPU. Call before starting any thread.
pub fn reserve_dsp_core() {
    let Some(dsp) = dsp_cpu() else {
        return;
    };
    let (rest, _) = plan_cpus(process_cpus(), dsp, smt_siblings(dsp).as_deref());
    if rest.is_empty() {
        return;
    }
    // SAFETY: a CPU set built and applied to the calling thread.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        for c in rest {
            libc::CPU_SET(c, &mut set);
        }
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
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
                let core = core_note(cpu);
                if core.contains("outside this process") {
                    tracing::warn!("DSP thread on CPU {cpu}: {core}");
                }
                notes.push(format!("CPU {cpu}, {core}"));
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
    fn the_dsp_core_is_kept_free() {
        assert_eq!(parse_cpu_list("3,27\n"), [3, 27]);
        assert_eq!(parse_cpu_list("0-2,8"), [0, 1, 2, 8]);
        assert!(parse_cpu_list("").is_empty());
        // A whole core (CPU 7 and 3) among others: the rest keep off both.
        let (rest, note) = plan_cpus(&[0, 1, 2, 3, 4, 5, 6, 7], 7, Some(&[3]));
        assert_eq!(rest, [0, 1, 2, 4, 5, 6]);
        assert!(note.contains("its other CPUs kept free"), "{note}");
        // The sibling is not ours (the first hardware round's node).
        let (rest, note) = plan_cpus(&[20, 21, 22, 23], 23, Some(&[47]));
        assert_eq!(rest, [20, 21, 22]);
        assert!(note.contains("CPU 47, outside this process"), "{note}");
        // Only one core: the other threads share it, off the DSP CPU.
        let (rest, note) = plan_cpus(&[3, 7], 7, Some(&[3]));
        assert_eq!(rest, [3]);
        assert!(note.contains("no other core"), "{note}");
        // No SMT, or no sysfs.
        assert!(plan_cpus(&[0, 1], 1, Some(&[])).1.contains("no SMT"));
        assert_eq!(plan_cpus(&[0, 1], 1, None).0, [0]);
    }

    #[test]
    fn workers_leave_a_core() {
        assert!(worker_threads() >= 1);
    }
}
