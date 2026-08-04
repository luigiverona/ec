use crate::{click, Error, Result};
use fs2::FileExt;
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::ffi::{OsStrExt, OsStringExt},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

const WORKER_MARKER: &str = "EC_INTERNAL_WORKER";
const WORKER_CPS: &str = "EC_INTERNAL_CPS";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    pub pid: u32,
    pub start_ticks: u64,
    pub executable: PathBuf,
    pub cps: u32,
    pub device: String,
}
pub struct WorkerLock {
    _file: File,
}
pub fn dir() -> Result<PathBuf> {
    let base = env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        Error::Message(
            "XDG_RUNTIME_DIR is not set; a secure per-user runtime directory is required".into(),
        )
    })?;
    let p = PathBuf::from(base).join("ec");
    if p.exists() {
        let m = fs::symlink_metadata(&p).map_err(|source| Error::Io {
            operation: "inspect runtime directory",
            path: p.display().to_string(),
            source,
        })?;
        if m.file_type().is_symlink() || !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
            return Err(Error::Message(format!(
                "unsafe runtime directory {} (must be a real directory owned by the current user)",
                p.display()
            )));
        }
    } else {
        fs::create_dir(&p).map_err(|source| Error::Io {
            operation: "create runtime directory",
            path: p.display().to_string(),
            source,
        })?
    }
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).map_err(|source| Error::Io {
        operation: "secure runtime directory",
        path: p.display().to_string(),
        source,
    })?;
    Ok(p)
}
fn lock_path(d: &Path) -> PathBuf {
    d.join("ec.lock")
}
fn status_path(d: &Path) -> PathBuf {
    d.join("ec.status")
}
pub fn acquire(d: &Path) -> Result<WorkerLock> {
    let p = lock_path(d);
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&p)
        .map_err(|source| Error::Io {
            operation: "open worker lock",
            path: p.display().to_string(),
            source,
        })?;
    f.try_lock_exclusive()
        .map_err(|_| Error::Message("EC is already running for this user".into()))?;
    Ok(WorkerLock { _file: f })
}
fn proc_ticks(pid: u32) -> Option<u64> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let end = s.rfind(')')?;
    s[end + 2..].split_whitespace().nth(19)?.parse().ok()
}
fn process_matches(s: &Status) -> bool {
    proc_ticks(s.pid) == Some(s.start_ticks)
        && fs::read_link(format!("/proc/{}/exe", s.pid))
            .ok()
            .and_then(|p| p.canonicalize().ok())
            == s.executable.canonicalize().ok()
        && fs::read(format!("/proc/{}/environ", s.pid))
            .ok()
            .is_some_and(|b| {
                b.split(|x| *x == 0)
                    .any(|x| x == format!("{WORKER_MARKER}=1").as_bytes())
            })
}
pub fn read_status(d: &Path) -> Result<Option<Status>> {
    let p = status_path(d);
    let bytes = match fs::read(&p) {
        Ok(x) => x,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::Io {
                operation: "read runtime status",
                path: p.display().to_string(),
                source,
            })
        }
    };
    let Some(s) = decode_status(&bytes) else {
        let _ = fs::remove_file(p);
        return Ok(None);
    };
    if process_matches(&s) {
        Ok(Some(s))
    } else {
        let _ = fs::remove_file(p);
        Ok(None)
    }
}
pub fn write_status(d: &Path, s: &Status) -> Result<()> {
    let p = status_path(d);
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&p)
        .map_err(|source| Error::Io {
            operation: "write runtime status",
            path: p.display().to_string(),
            source,
        })?;
    f.write_all(&encode_status(s)).map_err(|source| Error::Io {
        operation: "write runtime status",
        path: p.display().to_string(),
        source,
    })?;
    f.sync_all().map_err(|source| Error::Io {
        operation: "sync runtime status",
        path: p.display().to_string(),
        source,
    })
}
pub fn current_identity(cps: u32, device: String) -> Result<Status> {
    let pid = std::process::id();
    Ok(Status {
        pid,
        start_ticks: proc_ticks(pid)
            .ok_or_else(|| Error::Message("could not read current process identity".into()))?,
        executable: env::current_exe().map_err(|source| Error::Io {
            operation: "resolve executable",
            path: "/proc/self/exe".into(),
            source,
        })?,
        cps,
        device,
    })
}
pub fn clear(d: &Path) {
    let _ = fs::remove_file(status_path(d));
    let _ = fs::remove_file(d.join("ec.pid"));
}
pub fn start_background(cps: u32) -> Result<()> {
    let d = dir()?;
    if read_status(&d)?.is_some() {
        return Err(Error::Message(
            "EC is already running. Run `ec stop` first.".into(),
        ));
    }
    let exe = env::current_exe().map_err(|source| Error::Io {
        operation: "resolve executable",
        path: "/proc/self/exe".into(),
        source,
    })?;
    let log_path = d.join("ec.log");
    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|source| Error::Io {
            operation: "open background log",
            path: log_path.display().to_string(),
            source,
        })?;
    let err = log.try_clone().map_err(|source| Error::Io {
        operation: "clone background log",
        path: log_path.display().to_string(),
        source,
    })?;
    let mut child = Command::new(exe)
        .env(WORKER_MARKER, "1")
        .env(WORKER_CPS, cps.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err)
        .spawn()
        .map_err(|source| Error::Io {
            operation: "start background worker",
            path: "ec".into(),
            source,
        })?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(s) = read_status(&d)? {
            if s.pid == child.id() {
                return Ok(());
            }
        }
        if let Some(exit) = child
            .try_wait()
            .map_err(|e| Error::Message(format!("failed to inspect worker: {e}")))?
        {
            return Err(Error::Message(format!(
                "background worker exited during startup ({exit}); see {}",
                log_path.display()
            )));
        }
        thread::sleep(Duration::from_millis(25))
    }
    Err(Error::Message(format!(
        "background worker did not report ready within 5 seconds; see {}",
        log_path.display()
    )))
}

pub fn worker_cps() -> Result<Option<u32>> {
    if env::var_os(WORKER_MARKER).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(None);
    }
    let value = env::var(WORKER_CPS)
        .map_err(|_| Error::Message("internal worker CPS is missing".into()))?;
    let cps = value
        .parse::<u32>()
        .map_err(|_| Error::Message("internal worker CPS is invalid".into()))?;
    if !click::valid_cps(cps) {
        return Err(Error::Message(click::cps_error()));
    }
    Ok(Some(cps))
}

fn encode_status(status: &Status) -> Vec<u8> {
    let mut bytes = format!(
        "EC2\0{}\0{}\0{}\0",
        status.pid, status.start_ticks, status.cps
    )
    .into_bytes();
    bytes.extend_from_slice(status.executable.as_os_str().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(status.device.as_bytes());
    bytes
}

fn decode_status(bytes: &[u8]) -> Option<Status> {
    let mut fields = bytes.splitn(6, |byte| *byte == 0);
    if fields.next()? != b"EC2" {
        return None;
    }
    Some(Status {
        pid: parse_number(fields.next()?)?,
        start_ticks: parse_number(fields.next()?)?,
        cps: parse_number(fields.next()?)?,
        executable: PathBuf::from(OsString::from_vec(fields.next()?.to_vec())),
        device: std::str::from_utf8(fields.next()?).ok()?.to_string(),
    })
}

fn parse_number<T: std::str::FromStr>(field: &[u8]) -> Option<T> {
    std::str::from_utf8(field).ok()?.parse().ok()
}
pub fn stop() -> Result<bool> {
    let d = dir()?;
    let Some(s) = read_status(&d)? else {
        return Ok(false);
    };
    if !process_matches(&s) {
        clear(&d);
        return Ok(false);
    };
    if unsafe { libc::kill(s.pid as i32, libc::SIGTERM) } != 0 {
        return Err(Error::Message(format!(
            "failed to signal EC worker {}: {}",
            s.pid,
            std::io::Error::last_os_error()
        )));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !process_matches(&s) {
            clear(&d);
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50))
    }
    Err(Error::Message(format!(
        "EC worker {} did not stop within 5 seconds",
        s.pid
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_process_is_stale() {
        let s = Status {
            pid: u32::MAX,
            start_ticks: 1,
            executable: "/nope".into(),
            cps: click::DEFAULT_CPS,
            device: "x".into(),
        };
        assert!(!process_matches(&s));
    }

    #[test]
    fn reused_pid_without_worker_identity_is_rejected() {
        let pid = std::process::id();
        let status = Status {
            pid,
            start_ticks: proc_ticks(pid).unwrap(),
            executable: env::current_exe().unwrap(),
            cps: click::DEFAULT_CPS,
            device: "Mouse".into(),
        };
        assert!(!process_matches(&status));
    }

    #[test]
    fn duplicate_lock_is_rejected() {
        let directory = env::temp_dir().join(format!("ec-lock-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let first = acquire(&directory).unwrap();
        assert!(acquire(&directory).is_err());
        drop(first);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_status_is_removed_as_stale() {
        let directory = env::temp_dir().join(format!("ec-status-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(status_path(&directory), b"old or corrupt state").unwrap();
        assert!(read_status(&directory).unwrap().is_none());
        assert!(!status_path(&directory).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn status_roundtrip_preserves_identity() {
        let status = Status {
            pid: 42,
            start_ticks: 99,
            executable: "/path with spaces/ec".into(),
            cps: click::MAX_CPS,
            device: "Example Mouse".into(),
        };
        assert_eq!(decode_status(&encode_status(&status)), Some(status));
    }
}
