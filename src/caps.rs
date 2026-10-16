//! Process limits at start (§18). The image gives the binary `CAP_SYS_NICE` as a
//! permitted file capability only (`setcap cap_sys_nice+p`): a runtime that grants
//! `SYS_NICE` (Kubernetes `capabilities.add`) lets it into the permitted set, and
//! [`raise_permitted`] moves it into the effective set, so the uid-1000 process
//! can set `SCHED_FIFO`. Without the effective bit on the file, a runtime that
//! does not grant it still starts the mixer (it then runs `SCHED_OTHER`).

#[repr(C)]
struct Header {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Data {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// `_LINUX_CAPABILITY_VERSION_3`: two 32-bit words per set.
const VERSION_3: u32 = 0x2008_0522;

/// The capability names by bit number (`capability.h`).
const NAMES: [&str; 41] = [
    "chown",
    "dac_override",
    "dac_read_search",
    "fowner",
    "fsetid",
    "kill",
    "setgid",
    "setuid",
    "setpcap",
    "linux_immutable",
    "net_bind_service",
    "net_broadcast",
    "net_admin",
    "net_raw",
    "ipc_lock",
    "ipc_owner",
    "sys_module",
    "sys_rawio",
    "sys_chroot",
    "sys_ptrace",
    "sys_pacct",
    "sys_admin",
    "sys_boot",
    "sys_nice",
    "sys_resource",
    "sys_time",
    "sys_tty_config",
    "mknod",
    "lease",
    "audit_write",
    "audit_control",
    "setfcap",
    "mac_override",
    "mac_admin",
    "syslog",
    "wake_alarm",
    "block_suspend",
    "audit_read",
    "perfmon",
    "bpf",
    "checkpoint_restore",
];

fn get() -> std::io::Result<[Data; 2]> {
    let mut header = Header {
        version: VERSION_3,
        pid: 0,
    };
    let mut data = [Data::default(); 2];
    // SAFETY: capget with a v3 header and room for two data words.
    let rc = unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(data)
}

/// Make every permitted capability effective for this thread and the threads it
/// starts afterwards. Call before any thread starts.
pub fn raise_permitted() -> std::io::Result<()> {
    let mut data = get()?;
    if data.iter().all(|d| d.effective == d.permitted) {
        return Ok(());
    }
    for d in &mut data {
        d.effective = d.permitted;
    }
    let mut header = Header {
        version: VERSION_3,
        pid: 0,
    };
    // SAFETY: capset with a v3 header and two data words read by capget.
    let rc = unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn names(words: [u32; 2]) -> Vec<String> {
    (0..64)
        .filter(|bit| words[bit / 32] & (1 << (bit % 32)) != 0)
        .map(|bit| {
            NAMES
                .get(bit)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("cap_{bit}"))
        })
        .collect()
}

/// The effective capabilities of the calling thread, e.g. `sys_nice`; `none`.
pub fn effective() -> String {
    match get() {
        Ok(d) => {
            let list = names([d[0].effective, d[1].effective]);
            if list.is_empty() {
                "none".into()
            } else {
                list.join(",")
            }
        }
        Err(e) => format!("unknown ({e})"),
    }
}

/// Raise the soft limit of open files to the hard limit (16 inputs and their
/// MXL files fit in 1024, 256 might not). Returns the soft limit now in force.
pub fn raise_open_files() -> std::io::Result<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: plain getrlimit/setrlimit on a valid struct.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if limit.rlim_cur < limit.rlim_max {
            limit.rlim_cur = limit.rlim_max;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(limit.rlim_cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_names() {
        assert_eq!(names([1 << 23, 0]), ["sys_nice"]);
        assert_eq!(names([1 << 14, 1 << 8]), ["ipc_lock", "checkpoint_restore"]);
        assert_eq!(names([0, 1 << 20]), ["cap_52"]);
        assert!(names([0, 0]).is_empty());
        // The calls work on this kernel; raising changes nothing for a process
        // whose effective set already equals its permitted set.
        assert!(get().is_ok());
        assert!(raise_permitted().is_ok());
        assert!(!effective().starts_with("unknown"));
    }

    #[test]
    fn open_files_soft_limit_is_the_hard_limit() {
        let soft = raise_open_files().unwrap();
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        assert_eq!(soft, limit.rlim_cur);
        assert_eq!(limit.rlim_cur, limit.rlim_max);
    }
}
