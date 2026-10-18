//! Files that hold a secret: mode 0600 in a 0700 directory, written through
//! a temp file and rename, with symlinks refused.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The file's contents, or `None` when there is no file.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path);
    let mut file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Err(symlink(path)),
        Err(error) => return Err(error),
    };
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(Some(contents))
}

/// Replaces the file with `contents`, creating its directory with mode 0700
/// if it is missing. The directory must give no access to others.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = parent(path);
    DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let meta = fs::symlink_metadata(&dir)?;
    if meta.is_symlink() {
        return Err(symlink(&dir));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "{} must have mode 0700",
            dir.display()
        )));
    }
    refuse_symlink(path)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} names no file", path.display())))?;
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let _ = fs::remove_file(&temp);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(contents)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// Deletes the file, returning whether there was one.
pub fn remove(path: &Path) -> io::Result<bool> {
    refuse_symlink(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn refuse_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_symlink() => Err(symlink(path)),
        _ => Ok(()),
    }
}

fn symlink(path: &Path) -> io::Error {
    io::Error::other(format!("{} is a symbolic link", path.display()))
}
