use crate::{config, input, Error, Result};
use std::{
    fs,
    io::{self, BufRead, IsTerminal, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
};

pub const RULES: &str = include_str!("../assets/70-ec.rules");
const RULE_PATH: &str = "/etc/udev/rules.d/70-ec.rules";

pub fn run() -> Result<()> {
    if unsafe { libc::geteuid() } == 0 {
        root_setup(Path::new(RULE_PATH), &mut RealRunner)
    } else {
        user_setup()
    }
}

trait Runner {
    fn run(&mut self, args: &[&str]) -> io::Result<bool>;
}
struct RealRunner;
impl Runner for RealRunner {
    fn run(&mut self, args: &[&str]) -> io::Result<bool> {
        Ok(Command::new("udevadm").args(args).status()?.success())
    }
}

fn root_setup(target: &Path, runner: &mut dyn Runner) -> Result<()> {
    install_rule(target)?;
    let commands: &[&[&str]] = &[
        &["control", "--reload-rules"],
        &[
            "trigger",
            "--subsystem-match=misc",
            "--sysname-match=uinput",
        ],
        &["trigger", "--subsystem-match=input"],
        &["settle"],
    ];
    for args in commands {
        let ok = runner.run(args).map_err(|source| Error::Io {
            operation: "run udevadm",
            path: format!("udevadm {}", args.join(" ")),
            source,
        })?;
        if !ok {
            return Err(Error::Message(format!("udevadm {} failed", args.join(" "))));
        }
    }
    println!("System device access configured.\n\nReturn to your normal user and run:\n  ec setup\n\nA mouse reconnect or logout/login may be required before the active-session ACL is refreshed.");
    Ok(())
}

fn install_rule(target: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(target) {
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(Error::Message(format!(
                "refusing unsafe udev rule target {}",
                target.display()
            )));
        }
        if fs::read(target).ok().as_deref() == Some(RULES.as_bytes()) {
            fs::set_permissions(target, fs::Permissions::from_mode(0o644))
                .map_err(|e| ioe("set udev rule mode", target, e))?;
            return Ok(());
        }
    }
    let directory = target
        .parent()
        .ok_or_else(|| Error::Message("invalid udev rule path".into()))?;
    let temporary = directory.join(format!(".70-ec.rules.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)
            .map_err(|e| ioe("create temporary udev rule", &temporary, e))?;
        file.write_all(RULES.as_bytes())
            .map_err(|e| ioe("write udev rule", &temporary, e))?;
        file.sync_all()
            .map_err(|e| ioe("sync udev rule", &temporary, e))?;
        fs::rename(&temporary, target).map_err(|e| ioe("install udev rule", target, e))?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o644))
            .map_err(|e| ioe("set udev rule mode", target, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn user_setup() -> Result<()> {
    if fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uinput")
        .is_err()
    {
        let exe = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|source| Error::Io {
                operation: "resolve current executable",
                path: "/proc/self/exe".into(),
                source,
            })?;
        return Err(Error::Message(format!("EC needs one-time system device access.\n\nRun:\n  sudo {} setup\n\nThen run:\n  ec setup", shell_quote(&exe.to_string_lossy()))));
    }
    if let Some(configured) = config::read()? {
        if let Ok(candidate) = input::validate_configured(&configured) {
            println!(
                "Selected mouse     {}\nDevice path        {}",
                candidate.name,
                candidate.path.display()
            );
            return Ok(());
        }
    }
    let candidates: Vec<_> = input::discover()
        .candidates
        .into_iter()
        .filter(|c| c.strong && config::validate_stable_path(&c.path).is_ok())
        .collect();
    if candidates.is_empty() {
        return Err(Error::Message("No readable compatible mouse with a stable /dev/input/by-id/*-event-mouse path was found.".into()));
    }
    let chosen = if candidates.len() == 1 {
        &candidates[0]
    } else {
        print_candidates(&candidates);
        if !io::stdin().is_terminal() {
            return Err(Error::Message(
                "Interactive selection is required; no configuration was changed.".into(),
            ));
        }
        print!("Select a mouse number (or press Enter to cancel): ");
        io::stdout()
            .flush()
            .map_err(|e| Error::Message(e.to_string()))?;
        let mut line = String::new();
        io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| Error::Message(e.to_string()))?;
        let value = line
            .strip_suffix('\n')
            .and_then(|v| v.strip_suffix('\r').or(Some(v)))
            .unwrap_or(&line);
        if value.is_empty() {
            return Err(Error::Message(
                "Mouse selection cancelled; no configuration was changed.".into(),
            ));
        }
        let number: usize = value.parse().map_err(|_| {
            Error::Message("Invalid selection; no configuration was changed.".into())
        })?;
        candidates
            .get(number.wrapping_sub(1))
            .filter(|_| number > 0)
            .ok_or_else(|| {
                Error::Message("Invalid selection; no configuration was changed.".into())
            })?
    };
    config::write(&chosen.path)?;
    println!(
        "Selected mouse     {}\nDevice path        {}",
        chosen.name,
        chosen.path.display()
    );
    Ok(())
}

fn print_candidates(candidates: &[input::Candidate]) {
    println!("Compatible mice:");
    for (index, candidate) in candidates.iter().enumerate() {
        println!(
            "  {}. {}\n     {}",
            index + 1,
            candidate.name,
            candidate.path.display()
        );
    }
}

pub(crate) fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
fn ioe(operation: &'static str, path: &Path, source: io::Error) -> Error {
    Error::Io {
        operation,
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{symlink, MetadataExt},
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn temporary() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ec-setup-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    #[derive(Default)]
    struct FakeRunner {
        calls: Vec<Vec<String>>,
        fail_at: Option<usize>,
    }
    impl Runner for FakeRunner {
        fn run(&mut self, args: &[&str]) -> io::Result<bool> {
            self.calls
                .push(args.iter().map(|x| x.to_string()).collect());
            Ok(self.fail_at != Some(self.calls.len()))
        }
    }
    #[test]
    fn canonical_rule_is_narrow() {
        assert!(RULES.ends_with('\n'));
        assert!(RULES.contains("static_node=uinput"));
        assert!(RULES.contains("TAG+=\"uaccess\""));
        assert!(RULES.contains("ID_INPUT_MOUSE"));
        assert!(RULES.contains("ID_INPUT_TOUCHPAD"));
        for forbidden in ["MODE=", "0666", "GROUP=", "KEYBOARD"] {
            assert!(!RULES.contains(forbidden));
        }
    }
    #[test]
    fn quotes_executable_safely() {
        assert_eq!(shell_quote("/home/og/ec"), "/home/og/ec");
        assert_eq!(shell_quote("/a b/ec's"), "'/a b/ec'\\''s'");
    }
    #[test]
    fn rule_install_is_atomic_mode_0644_and_idempotent() {
        let dir = temporary();
        let target = dir.join("70-ec.rules");
        fs::write(&target, "old\n").unwrap();
        install_rule(&target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), RULES);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let inode = fs::metadata(&target).unwrap().ino();
        install_rule(&target).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().ino(), inode);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unsafe_rule_targets_are_rejected() {
        let dir = temporary();
        let real = dir.join("real");
        fs::write(&real, "safe").unwrap();
        let link = dir.join("link");
        symlink(&real, &link).unwrap();
        assert!(install_rule(&link).is_err());
        assert_eq!(fs::read_to_string(&real).unwrap(), "safe");
        assert!(install_rule(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn root_setup_runs_exact_commands_and_propagates_failure() {
        let dir = temporary();
        let target = dir.join("70-ec.rules");
        let mut runner = FakeRunner::default();
        root_setup(&target, &mut runner).unwrap();
        assert_eq!(
            runner.calls,
            vec![
                vec!["control", "--reload-rules"],
                vec![
                    "trigger",
                    "--subsystem-match=misc",
                    "--sysname-match=uinput"
                ],
                vec!["trigger", "--subsystem-match=input"],
                vec!["settle"]
            ]
        );
        let mut failing = FakeRunner {
            fail_at: Some(2),
            ..Default::default()
        };
        assert!(root_setup(&target, &mut failing).is_err());
        assert_eq!(failing.calls.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
