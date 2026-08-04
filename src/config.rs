use crate::{Error, Result};
use std::{
    env, fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

pub fn path() -> Result<PathBuf> {
    path_for(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

fn path_for(xdg: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> Result<PathBuf> {
    let base = match xdg {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(home.ok_or_else(|| {
            Error::Message("HOME is unset; cannot locate EC configuration".into())
        })?)
        .join(".config"),
    };
    Ok(base.join("ec/device"))
}

pub fn validate_stable_path(path: &Path) -> Result<()> {
    let mut parts = path.components();
    if parts.next() != Some(Component::RootDir)
        || parts.next() != Some(Component::Normal("dev".as_ref()))
        || parts.next() != Some(Component::Normal("input".as_ref()))
        || parts.next() != Some(Component::Normal("by-id".as_ref()))
        || parts.next().is_none()
        || parts.next().is_some()
        || !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with("-event-mouse"))
    {
        return Err(Error::Message(
            "configured mouse path must be an absolute /dev/input/by-id/*-event-mouse path".into(),
        ));
    }
    Ok(())
}

pub fn read() -> Result<Option<PathBuf>> {
    let target = path()?;
    read_from(&target)
}

fn read_from(target: &Path) -> Result<Option<PathBuf>> {
    let directory = target
        .parent()
        .ok_or_else(|| Error::Message("invalid configuration path".into()))?;
    match fs::symlink_metadata(directory) {
        Ok(meta)
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() } =>
        {
            return Err(Error::Message(format!(
                "unsafe configuration directory {}",
                directory.display()
            )))
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io("inspect configuration directory", directory, error)),
    }
    let meta = match fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io("inspect device configuration", target, source)),
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err(Error::Message(format!(
            "unsafe device configuration {} (must be a regular file owned by the current user)",
            target.display()
        )));
    }
    let bytes = fs::read(target).map_err(|e| io("read device configuration", target, e))?;
    if bytes.contains(&0) {
        return Err(Error::Message(
            "device configuration contains a NUL byte".into(),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::Message("device configuration is not UTF-8".into()))?;
    if !text.ends_with('\n') || text.lines().count() != 1 || text.trim().is_empty() {
        return Err(Error::Message(
            "device configuration must contain exactly one nonempty path and a final newline"
                .into(),
        ));
    }
    let selected = PathBuf::from(text.trim_end_matches('\n'));
    validate_stable_path(&selected)?;
    Ok(Some(selected))
}

pub fn write(selected: &Path) -> Result<()> {
    validate_stable_path(selected)?;
    let target = path()?;
    write_to(&target, selected)
}

fn write_to(target: &Path, selected: &Path) -> Result<()> {
    validate_stable_path(selected)?;
    let directory = target.parent().unwrap();
    ensure_directory(directory)?;
    if let Ok(meta) = fs::symlink_metadata(target) {
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(Error::Message(format!(
                "unsafe device configuration {}",
                target.display()
            )));
        }
        if fs::read(target).ok().as_deref() == Some(format!("{}\n", selected.display()).as_bytes())
        {
            return Ok(());
        }
    }
    let temporary = directory.join(format!(".device.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| io("create temporary device configuration", &temporary, e))?;
        writeln!(file, "{}", selected.display())
            .map_err(|e| io("write device configuration", &temporary, e))?;
        file.sync_all()
            .map_err(|e| io("sync device configuration", &temporary, e))?;
        fs::rename(&temporary, target)
            .map_err(|e| io("install device configuration", target, e))?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o600))
            .map_err(|e| io("secure device configuration", target, e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_directory(directory: &Path) -> Result<()> {
    let parent = directory
        .parent()
        .ok_or_else(|| Error::Message("invalid configuration directory".into()))?;
    if !parent.is_dir() {
        return Err(Error::Message(format!(
            "configuration parent {} is not a directory",
            parent.display()
        )));
    }
    match fs::symlink_metadata(directory) {
        Ok(meta)
            if meta.file_type().is_symlink()
                || !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() } =>
        {
            return Err(Error::Message(format!(
                "unsafe configuration directory {}",
                directory.display()
            )))
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(directory)
            .map_err(|e| io("create configuration directory", directory, e))?,
        Err(e) => return Err(io("inspect configuration directory", directory, e)),
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|e| io("secure configuration directory", directory, e))
}

fn io(operation: &'static str, path: &Path, source: std::io::Error) -> Error {
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
        os::unix::fs::symlink,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn fixture() -> (PathBuf, PathBuf) {
        let root = env::temp_dir().join(format!(
            "ec-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        (root.clone(), root.join("ec/device"))
    }
    #[test]
    fn stable_paths_are_strict() {
        assert!(validate_stable_path(Path::new("/dev/input/by-id/usb-x-event-mouse")).is_ok());
        for bad in [
            "dev/input/by-id/x-event-mouse",
            "/dev/input/event2",
            "/dev/input/by-id/x",
            "/dev/input/by-id/a/usb-x-event-mouse",
        ] {
            assert!(validate_stable_path(Path::new(bad)).is_err(), "{bad}");
        }
    }
    #[test]
    fn resolves_xdg_then_home_fallback() {
        assert_eq!(
            path_for(Some("/xdg".into()), Some("/home/me".into())).unwrap(),
            PathBuf::from("/xdg/ec/device")
        );
        assert_eq!(
            path_for(None, Some("/home/me".into())).unwrap(),
            PathBuf::from("/home/me/.config/ec/device")
        );
        assert_eq!(
            path_for(Some("".into()), Some("/home/me".into())).unwrap(),
            PathBuf::from("/home/me/.config/ec/device")
        );
        assert!(path_for(None, None).is_err());
    }
    #[test]
    fn secure_roundtrip_modes_and_idempotence() {
        let (root, target) = fixture();
        let selected = Path::new("/dev/input/by-id/usb-x-event-mouse");
        write_to(&target, selected).unwrap();
        assert_eq!(read_from(&target).unwrap(), Some(selected.into()));
        assert_eq!(
            fs::metadata(target.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let inode = fs::metadata(&target).unwrap().ino();
        write_to(&target, selected).unwrap();
        assert_eq!(fs::metadata(&target).unwrap().ino(), inode);
        assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_and_multiline_files_are_rejected() {
        let (root, target) = fixture();
        fs::create_dir(target.parent().unwrap()).unwrap();
        for content in [
            b"/dev/input/by-id/x-event-mouse".as_slice(),
            b"/dev/input/by-id/x-event-mouse\nextra\n",
            b"/dev/input/by-id/x-event-mouse\0\n",
            b"\xff\n",
        ] {
            fs::write(&target, content).unwrap();
            assert!(read_from(&target).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlinks_and_nonregular_targets_are_rejected() {
        let (root, target) = fixture();
        let real_dir = root.join("real");
        fs::create_dir(&real_dir).unwrap();
        symlink(&real_dir, target.parent().unwrap()).unwrap();
        assert!(read_from(&target).is_err());
        assert!(write_to(&target, Path::new("/dev/input/by-id/x-event-mouse")).is_err());
        fs::remove_file(target.parent().unwrap()).unwrap();
        fs::create_dir(target.parent().unwrap()).unwrap();
        let real = root.join("real-device");
        fs::write(&real, "unchanged").unwrap();
        symlink(&real, &target).unwrap();
        assert!(write_to(&target, Path::new("/dev/input/by-id/x-event-mouse")).is_err());
        assert_eq!(fs::read_to_string(real).unwrap(), "unchanged");
        fs::remove_dir_all(root).unwrap();
    }
}
