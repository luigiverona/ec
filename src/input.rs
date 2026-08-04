use crate::{
    click::{FrameSink, Scheduler, SchedulerCommand},
    Error, Result,
};
use evdev::{
    uinput::VirtualDevice, AttributeSet, BusType, Device, EventType, InputEvent, InputId, KeyCode,
    RelativeAxisCode,
};
use std::{
    fs,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};

pub const VIRTUAL_NAME: &str = "EC Virtual Mouse";
const EC_VENDOR: u16 = 0x4543;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub name: String,
    pub strong: bool,
}
const DIAGNOSTIC_LIMIT: usize = 16;
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub input_missing: bool,
    pub event_nodes: usize,
    pub candidates: Vec<Candidate>,
    pub incompatible: Vec<PathBuf>,
    pub permission_denied: Vec<PathBuf>,
    pub open_failures: Vec<(PathBuf, String)>,
}
pub fn is_candidate(
    name: &str,
    has_relative_xy: bool,
    has_left_button: bool,
    has_absolute_axes: bool,
) -> bool {
    let lower = name.to_ascii_lowercase();
    name != VIRTUAL_NAME
        && !lower.contains("touchpad")
        && !lower.contains("trackpad")
        && has_relative_xy
        && has_left_button
        && !has_absolute_axes
}
fn is_ec_identity(device: &Device) -> bool {
    let id = device.input_id();
    device.name() == Some(VIRTUAL_NAME)
        || (id.bus_type() == BusType::BUS_VIRTUAL && id.vendor() == EC_VENDOR)
}
pub fn discover() -> Discovery {
    let mut out = Discovery::default();
    let Ok(entries) = fs::read_dir("/dev/input") else {
        out.input_missing = true;
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        out.event_nodes += 1;
        match Device::open(&p) {
            Ok(d) => {
                let name = d.name().unwrap_or("Unknown mouse").to_string();
                let rel = d.supported_relative_axes().is_some_and(|s| {
                    s.contains(RelativeAxisCode::REL_X) && s.contains(RelativeAxisCode::REL_Y)
                });
                let btn = d
                    .supported_keys()
                    .is_some_and(|s| s.contains(KeyCode::BTN_LEFT));
                let absolute = d
                    .supported_absolute_axes()
                    .is_some_and(|axes| axes.iter().next().is_some());
                let keyboard = d
                    .supported_keys()
                    .is_some_and(|keys| keys.contains(KeyCode::KEY_A));
                if is_candidate(&name, rel, btn, absolute) && !is_ec_identity(&d) {
                    let stable = stable_mouse_path(&p);
                    out.candidates.push(Candidate {
                        path: stable.clone().unwrap_or(p),
                        name,
                        strong: stable.is_some()
                            || (!keyboard
                                && d.name()
                                    .is_some_and(|n| n.to_ascii_lowercase().contains("mouse"))),
                    })
                } else if out.incompatible.len() < DIAGNOSTIC_LIMIT {
                    out.incompatible.push(p);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                if out.permission_denied.len() < DIAGNOSTIC_LIMIT {
                    out.permission_denied.push(p);
                }
            }
            Err(error) => {
                if out.open_failures.len() < DIAGNOSTIC_LIMIT {
                    out.open_failures.push((p, error.to_string()));
                }
            }
        }
    }
    out.candidates.sort_by(|a, b| a.path.cmp(&b.path));
    out
}
fn stable_mouse_path(event_path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(event_path).ok()?;
    fs::read_dir("/dev/input/by-id")
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with("-event-mouse")
        })
        .find_map(|entry| {
            let path = entry.path();
            (fs::canonicalize(&path).ok()? == canonical).then_some(path)
        })
}
pub fn select() -> Result<Candidate> {
    if let Some(configured) = crate::config::read()? {
        return validate_configured(&configured).map_err(|error| Error::Message(format!(
            "The configured mouse is unavailable or no longer compatible.\nRun:\n  ec setup\n\n{error}"
        )));
    }
    let discovery = discover();
    let c = &discovery.candidates;
    if let Some(candidate) = choose(c) {
        return Ok(candidate);
    }
    let list = if c.is_empty() {
        if discovery.input_missing {
            "/dev/input is missing.".into()
        } else if discovery.event_nodes == 0 {
            "No input event nodes were found.".into()
        } else if !discovery.permission_denied.is_empty() && discovery.incompatible.is_empty() {
            format!(
                "Input event nodes are present but unreadable.\nPermission denied:\n{}",
                discovery
                    .permission_denied
                    .iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            let mut message =
                "Readable input devices were found, but none is a compatible physical mouse."
                    .to_string();
            if !discovery.permission_denied.is_empty() {
                message.push_str("\nSome event nodes were also inaccessible:\n");
                message.push_str(
                    &discovery
                        .permission_denied
                        .iter()
                        .map(|p| format!("  {}", p.display()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            if !discovery.open_failures.is_empty() {
                message.push_str("\nOther event-node open failures:\n");
                message.push_str(
                    &discovery
                        .open_failures
                        .iter()
                        .map(|(p, e)| format!("  {}: {e}", p.display()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            message
        }
    } else {
        format!(
            "Candidate devices:\n{}",
            c.iter()
                .map(|x| format!("  {}  {}", x.path.display(), x.name))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Err(Error::Message(format!(
        "{list}\n{}",
        if c.len() > 1 {
            "Multiple compatible mice were found.\nRun:\n  ec setup"
        } else {
            "EC could not select one physical mouse safely.\nRun:\n  ec setup"
        }
    )))
}

pub fn validate_configured(path: &Path) -> Result<Candidate> {
    crate::config::validate_stable_path(path)?;
    let target = fs::canonicalize(path).map_err(|source| Error::Io {
        operation: "resolve configured mouse",
        path: path.display().to_string(),
        source,
    })?;
    if target.parent() != Some(Path::new("/dev/input"))
        || !target
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("event"))
    {
        return Err(Error::Message(format!(
            "configured mouse {} does not resolve to a /dev/input/event* node",
            path.display()
        )));
    }
    let device = Device::open(path).map_err(|source| Error::Io {
        operation: "open configured mouse",
        path: path.display().to_string(),
        source,
    })?;
    candidate_from_device(path.to_path_buf(), &device).ok_or_else(|| {
        Error::Message(format!(
            "configured mouse {} is not a compatible physical mouse",
            path.display()
        ))
    })
}

fn candidate_from_device(path: PathBuf, d: &Device) -> Option<Candidate> {
    let name = d.name().unwrap_or("Unknown mouse").to_string();
    let rel = d.supported_relative_axes().is_some_and(|s| {
        s.contains(RelativeAxisCode::REL_X) && s.contains(RelativeAxisCode::REL_Y)
    });
    let btn = d
        .supported_keys()
        .is_some_and(|s| s.contains(KeyCode::BTN_LEFT));
    let absolute = d
        .supported_absolute_axes()
        .is_some_and(|axes| axes.iter().next().is_some());
    (is_candidate(&name, rel, btn, absolute) && !is_ec_identity(d)).then_some(Candidate {
        path,
        name,
        strong: true,
    })
}

fn choose(candidates: &[Candidate]) -> Option<Candidate> {
    let mut strong = candidates.iter().filter(|candidate| candidate.strong);
    let candidate = strong.next()?;
    strong.next().is_none().then(|| candidate.clone())
}

struct VirtualFrameSink {
    dev: Arc<Mutex<VirtualDevice>>,
    key: KeyCode,
}

impl FrameSink for VirtualFrameSink {
    fn emit_button_frame(&mut self, down: bool) -> std::io::Result<()> {
        let mut dev = self
            .dev
            .lock()
            .map_err(|_| std::io::Error::other("virtual device lock poisoned"))?;
        dev.emit(&[InputEvent::new(
            EventType::KEY.0,
            self.key.0,
            i32::from(down),
        )])
    }
}

fn run_scheduler(
    rx: Receiver<SchedulerCommand>,
    dev: Arc<Mutex<VirtualDevice>>,
    key: KeyCode,
    cps: u32,
    stop: Arc<AtomicBool>,
) -> std::io::Result<()> {
    let mut scheduler = Scheduler::new(cps);
    let mut sink = VirtualFrameSink { dev, key };
    let result = (|| {
        loop {
            let msg = if let Some(deadline) = scheduler.deadline() {
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            } else {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match msg {
                Ok(command) => {
                    if !scheduler.command(command, Instant::now(), &mut sink)? {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    scheduler.advance(Instant::now(), &mut sink)?;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    scheduler.command(SchedulerCommand::Shutdown, Instant::now(), &mut sink)?;
                    break;
                }
            }
        }
        Ok(())
    })();
    if result.is_err() {
        // If an emission failed while the button was down, make one best-effort
        // release attempt while preserving the original scheduler error.
        let _ = scheduler.command(SchedulerCommand::Shutdown, Instant::now(), &mut sink);
    }
    stop.store(true, Ordering::SeqCst);
    result
}
fn send_bounded(tx: &SyncSender<SchedulerCommand>, command: SchedulerCommand) -> Result<()> {
    tx.send(command)
        .map_err(|e| Error::Message(format!("click scheduler unavailable: {e}")))
}
pub fn run(cps: u32, stop: Arc<AtomicBool>) -> Result<String> {
    run_ready(cps, stop, |_| Ok(()))
}
pub fn run_ready<F: FnOnce(&str) -> Result<()>>(
    cps: u32,
    stop: Arc<AtomicBool>,
    ready: F,
) -> Result<String> {
    if !crate::click::valid_cps(cps) {
        return Err(Error::Message(crate::click::cps_error()));
    }
    let selected = select()?;
    let mut physical = Device::open(&selected.path).map_err(|source| Error::Io {
        operation: "open input device",
        path: selected.path.display().to_string(),
        source,
    })?;
    physical.grab().map_err(|source| Error::Io {
        operation: "exclusively grab input device",
        path: selected.path.display().to_string(),
        source,
    })?;
    let key = KeyCode::BTN_LEFT;
    let keys: AttributeSet<KeyCode> = physical
        .supported_keys()
        .map(|values| values.iter().collect())
        .unwrap_or_else(AttributeSet::new);
    if !keys.contains(key) {
        return Err(Error::Message(format!(
            "input device {} does not support the required {} button",
            selected.path.display(),
            "left"
        )));
    }
    let rel: AttributeSet<RelativeAxisCode> = physical
        .supported_relative_axes()
        .map(|values| values.iter().collect())
        .unwrap_or_else(AttributeSet::new);
    let virtual_dev = VirtualDevice::builder()
        .map_err(|source| Error::Io {
            operation: "open /dev/uinput",
            path: "/dev/uinput".into(),
            source,
        })?
        .name(VIRTUAL_NAME)
        .input_id(InputId::new(BusType::BUS_VIRTUAL, EC_VENDOR, 1, 1))
        .with_keys(&keys)
        .map_err(|source| Error::Io {
            operation: "configure virtual mouse buttons",
            path: "/dev/uinput".into(),
            source,
        })?
        .with_relative_axes(&rel)
        .map_err(|source| Error::Io {
            operation: "configure virtual mouse axes",
            path: "/dev/uinput".into(),
            source,
        })?
        .build()
        .map_err(|source| Error::Io {
            operation: "create virtual mouse",
            path: "/dev/uinput".into(),
            source,
        })?;
    let dev = Arc::new(Mutex::new(virtual_dev));
    let (tx, rx) = mpsc::sync_channel(2);
    let sched_dev = Arc::clone(&dev);
    let sched_stop = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("ec-click-scheduler".into())
        .spawn(move || run_scheduler(rx, sched_dev, key, cps, sched_stop))
        .map_err(|e| Error::Message(format!("failed to start click scheduler: {e}")))?;
    if let Err(error) = ready(&selected.name) {
        let _ = tx.send(SchedulerCommand::Shutdown);
        let _ = handle.join();
        return Err(error);
    }
    let loop_result = (|| {
        while !stop.load(Ordering::SeqCst) {
            let mut pollfd = libc::pollfd {
                fd: physical.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let polled = unsafe { libc::poll(&mut pollfd, 1, 100) };
            if polled < 0 {
                let source = std::io::Error::last_os_error();
                if source.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(Error::Io {
                    operation: "wait for input events",
                    path: selected.path.display().to_string(),
                    source,
                });
            }
            if polled == 0 {
                continue;
            }
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(Error::Message(format!(
                    "input device {} disconnected",
                    selected.path.display()
                )));
            }
            let events = physical.fetch_events().map_err(|source| Error::Io {
                operation: "read events from input device",
                path: selected.path.display().to_string(),
                source,
            })?;
            let mut forwarded = Vec::new();
            for e in events {
                if e.event_type() == EventType::KEY && e.code() == key.0 {
                    if e.value() == 1 {
                        send_bounded(&tx, SchedulerCommand::Hold(true))?
                    } else if e.value() == 0 {
                        send_bounded(&tx, SchedulerCommand::Hold(false))?
                    }
                } else {
                    forwarded.push(e)
                }
            }
            if !forwarded.is_empty() {
                dev.lock()
                    .expect("virtual device lock poisoned")
                    .emit(&forwarded)
                    .map_err(|source| Error::Io {
                        operation: "forward mouse events",
                        path: "EC Virtual Mouse".into(),
                        source,
                    })?
            }
        }
        Ok(())
    })();
    let _ = tx.send(SchedulerCommand::Shutdown);
    let joined = handle
        .join()
        .map_err(|_| Error::Message("click scheduler panicked".into()))?;
    drop(physical);
    if let Err(source) = joined {
        return Err(Error::Io {
            operation: "emit synthetic mouse button frame",
            path: "EC Virtual Mouse".into(),
            source,
        });
    }
    loop_result?;
    Ok(selected.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excludes_virtual() {
        assert!(!is_candidate(VIRTUAL_NAME, true, true, false));
        assert!(!is_candidate("Keyboard", false, true, false));
        assert!(!is_candidate("Touchpad", true, true, false));
        assert!(!is_candidate("Tablet", true, true, true));
        assert!(is_candidate("Mouse", true, true, false));
    }

    #[test]
    fn ambiguous_candidates_are_refused() {
        let candidate = |path: &str| Candidate {
            path: path.into(),
            name: "Mouse".into(),
            strong: true,
        };
        assert!(choose(&[candidate("/dev/a"), candidate("/dev/b")]).is_none());
        assert_eq!(
            choose(&[candidate("/dev/a")]).map(|item| item.path),
            Some(PathBuf::from("/dev/a"))
        );
    }
}
