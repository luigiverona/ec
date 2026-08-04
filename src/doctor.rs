use crate::{input, runtime, Error, Result};
use std::{fs, path::Path};

#[derive(Debug)]
pub struct Readiness {
    pub linux: bool,
    pub session: &'static str,
    pub uinput: Check,
    pub device: std::result::Result<input::Candidate, String>,
    pub grab: Check,
    pub runtime: Check,
}

#[derive(Debug)]
pub enum Check {
    Ready(String),
    Failed(String),
}
impl Check {
    fn ok(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
    fn text(&self) -> &str {
        match self {
            Self::Ready(s) | Self::Failed(s) => s,
        }
    }
}

pub fn check() -> Readiness {
    let session = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "Wayland"
    } else if std::env::var_os("DISPLAY").is_some() {
        "X11"
    } else {
        "not detected"
    };
    let uinput = if !Path::new("/dev/uinput").exists() {
        Check::Failed("missing".into())
    } else {
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uinput")
        {
            Ok(_) => Check::Ready("available".into()),
            Err(e) => Check::Failed(format!("inaccessible: {e}")),
        }
    };
    let device = input::select().map_err(|e| e.to_string());
    let grab = match &device {
        Ok(candidate) => match evdev::Device::open(&candidate.path) {
            Ok(mut dev) => match dev.grab() {
                Ok(()) => match dev.ungrab() {
                    Ok(()) => Check::Ready("valid".into()),
                    Err(e) => Check::Failed(format!("failed to release test grab: {e}")),
                },
                Err(e) => Check::Failed(format!(
                    "cannot exclusively grab {}: {e}",
                    candidate.path.display()
                )),
            },
            Err(e) => Check::Failed(format!("cannot open {}: {e}", candidate.path.display())),
        },
        Err(_) => Check::Failed("not tested".into()),
    };
    let runtime = match runtime::dir().and_then(|d| runtime::read_status(&d).map(|s| s.is_some())) {
        Ok(true) => Check::Ready("ready (EC running)".into()),
        Ok(false) => Check::Ready("ready".into()),
        Err(e) => Check::Failed(e.to_string()),
    };
    Readiness {
        linux: cfg!(target_os = "linux"),
        session,
        uinput,
        device,
        grab,
        runtime,
    }
}

impl Readiness {
    pub fn ready(&self) -> bool {
        self.linux && self.uinput.ok() && self.device.is_ok() && self.grab.ok() && self.runtime.ok()
    }
}

pub fn render(report: &Readiness) {
    line("System", if report.linux { "Linux" } else { "unsupported" });
    line("Session", report.session);
    line("uinput", report.uinput.text());
    if !report.uinput.ok() {
        if let Ok(exe) = std::env::current_exe().and_then(fs::canonicalize) {
            println!(
                "  EC needs one-time system device access.\n  Run:\n    sudo {} setup",
                crate::setup::shell_quote(&exe.to_string_lossy())
            );
        }
    }
    match &report.device {
        Ok(candidate) => {
            line("Configured mouse", &candidate.name);
            line("Device path", &candidate.path.display().to_string());
        }
        Err(error) => {
            line("Input device", "not ready");
            println!("  {error}");
        }
    }
    line("Permissions", report.grab.text());
    line("Runtime", report.runtime.text());
    println!("\nEC is {}ready.", if report.ready() { "" } else { "not " });
}

pub fn run() -> bool {
    let report = check();
    render(&report);
    report.ready()
}

pub fn preflight() -> bool {
    let report = check();
    if !report.ready() {
        render(&report);
        false
    } else {
        true
    }
}

pub fn start_with<F>(report: Readiness, spawn: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    if !report.ready() {
        render(&report);
        return Err(Error::Message(
            "readiness preflight failed; EC was not started".into(),
        ));
    }
    spawn()
}

fn line(label: &str, value: &str) {
    println!("{label:<17} {value}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, path::PathBuf};
    fn report(ready: bool) -> Readiness {
        Readiness {
            linux: ready,
            session: "test",
            uinput: if ready {
                Check::Ready("available".into())
            } else {
                Check::Failed("denied".into())
            },
            device: Ok(input::Candidate {
                path: PathBuf::from("/dev/input/by-id/test-event-mouse"),
                name: "Test mouse".into(),
                strong: true,
            }),
            grab: Check::Ready("valid".into()),
            runtime: Check::Ready("ready".into()),
        }
    }
    #[test]
    fn failed_preflight_prevents_spawn_callback() {
        let called = Cell::new(false);
        assert!(start_with(report(false), || {
            called.set(true);
            Ok(())
        })
        .is_err());
        assert!(!called.get());
    }
    #[test]
    fn successful_preflight_permits_spawn_callback() {
        let called = Cell::new(false);
        start_with(report(true), || {
            called.set(true);
            Ok(())
        })
        .unwrap();
        assert!(called.get());
    }
}
