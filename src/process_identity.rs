use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ProcessIdentity {
    pid: u32,
    start_time: u64,
    scope: String,
}

enum Probe {
    Present(u64),
    Gone,
    Unknown,
}

impl ProcessIdentity {
    pub(crate) fn current() -> Option<Self> {
        let pid = std::process::id();
        let scope = platform::scope()?;
        match platform::probe(pid) {
            Probe::Present(start_time) => Some(Self {
                pid,
                start_time,
                scope,
            }),
            _ => None,
        }
    }

    pub(crate) fn confirms_exit(&self, owner: &Self) -> bool {
        if self.scope != owner.scope || owner.pid == 0 {
            return false;
        }
        match platform::probe(owner.pid) {
            Probe::Present(start_time) => start_time != owner.start_time,
            Probe::Gone => true,
            Probe::Unknown => false,
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::Probe;
    use std::{
        os::windows::io::{FromRawHandle, OwnedHandle},
        ptr::null_mut,
    };
    use windows_sys::{
        Win32::{
            Foundation::{
                ERROR_INVALID_PARAMETER, ERROR_SUCCESS, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT,
            },
            System::{
                Registry::{
                    HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
                },
                Threading::{
                    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                    PROCESS_SYNCHRONIZE, WaitForSingleObject,
                },
            },
        },
        w,
    };

    pub(super) fn scope() -> Option<String> {
        let mut buffer = [0u16; 64];
        let mut bytes = size_of_val(&buffer) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                w!(r"SOFTWARE\Microsoft\Cryptography"),
                w!("MachineGuid"),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let end = buffer.iter().position(|&c| c == 0)?;
        let guid = uuid::Uuid::parse_str(&String::from_utf16(&buffer[..end]).ok()?).ok()?;
        Some(format!("windows:{guid}"))
    }

    pub(super) fn probe(pid: u32) -> Probe {
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return if std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_INVALID_PARAMETER as i32)
            {
                Probe::Gone
            } else {
                Probe::Unknown
            };
        }
        let _handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
        if unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
            return Probe::Unknown;
        }
        match unsafe { WaitForSingleObject(raw, 0) } {
            WAIT_OBJECT_0 => Probe::Gone,
            WAIT_TIMEOUT => Probe::Present(
                u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime),
            ),
            _ => Probe::Unknown,
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Probe;
    use std::fs;

    pub(super) fn scope() -> Option<String> {
        let own_stat = fs::read_to_string("/proc/self/stat").ok()?;
        if own_stat
            .split_ascii_whitespace()
            .next()?
            .parse::<u32>()
            .ok()?
            != std::process::id()
        {
            return None;
        }
        let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
        let boot = uuid::Uuid::parse_str(boot.trim()).ok()?;
        let pid_namespace = fs::read_link("/proc/self/ns/pid").ok()?;
        // /proc/<pid>/stat start ticks are relative to the reader's time namespace.
        let time_namespace = fs::read_link("/proc/self/ns/time").ok()?;
        Some(format!(
            "linux:{boot}:{}:{}",
            pid_namespace.to_str()?,
            time_namespace.to_str()?
        ))
    }

    pub(super) fn probe(pid: u32) -> Probe {
        let Ok(pid) = i32::try_from(pid) else {
            return Probe::Unknown;
        };
        if pid <= 0 {
            return Probe::Unknown;
        }
        if let Ok(stat) = fs::read(format!("/proc/{pid}/stat"))
            && let Some(end) = stat.iter().rposition(|&b| b == b')')
            && let Ok(fields) = std::str::from_utf8(&stat[end + 1..])
        {
            let mut fields = fields.split_ascii_whitespace();
            match fields.next() {
                Some("Z" | "X" | "x") => return Probe::Gone,
                Some(_) => {
                    if let Some(start) = fields.nth(18).and_then(|n| n.parse().ok()) {
                        return Probe::Present(start);
                    }
                }
                None => (),
            }
        }
        // hidepid can turn permission errors into ENOENT. Only ESRCH from the kernel proves absence.
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            Probe::Gone
        } else {
            Probe::Unknown
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::Probe;

    pub(super) fn scope() -> Option<String> {
        None
    }
    pub(super) fn probe(_: u32) -> Probe {
        Probe::Unknown
    }
}
