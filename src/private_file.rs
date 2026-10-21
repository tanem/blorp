//! Files that hold a secret, written through a temp file and rename, with
//! symbolic links refused. On Unix the file is mode 0600 in a 0700
//! directory; on Windows its access list grants the current user alone.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The file's contents, or `None` when there is no file.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let mut file = match open_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(Some(contents))
}

/// Replaces the file with `contents`, creating its directory if it is
/// missing. On Unix the directory is created with mode 0700 and must give
/// no access to others.
pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = parent(path);
    create_dir(&dir)?;
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
    let written = create_private(&temp)
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

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => symlink(path),
            _ => error,
        })
}

#[cfg(unix)]
fn create_dir(dir: &Path) -> io::Result<()> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if meta.is_symlink() {
        return Err(symlink(dir));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "{} must have mode 0700",
            dir.display()
        )));
    }
    Ok(())
}

/// A new file with mode 0600.
#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Opens a link itself rather than its target, and refuses it.
#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(symlink(path));
    }
    Ok(file)
}

/// The directory's access list is left as created: the file's own access
/// list keeps others out.
#[cfg(windows)]
fn create_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    refuse_symlink(dir)
}

/// A new file whose access list grants the current user alone. No other
/// handle can open the file until its access list is set, since this one
/// shares no access.
#[cfg(windows)]
fn create_private(path: &Path) -> io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, WRITE_DAC};

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_WRITE | WRITE_DAC)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    windows::owner_only(&file)?;
    Ok(file)
}

#[cfg(windows)]
mod windows {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
        SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Replaces the file's access list with one that grants the current
    /// user full access and inherits nothing.
    pub fn owner_only(file: &File) -> io::Result<()> {
        let user = CurrentUser::new()?;
        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: user.sid().cast(),
            },
        };
        let mut acl: *mut ACL = null_mut();
        // SAFETY: `access` names a SID that `user` keeps alive, and `acl`
        // receives a new list that is freed below.
        let error = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        // SAFETY: the handle is open with WRITE_DAC, and `acl` is valid
        // until freed after the call.
        let error = unsafe {
            let error = SetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl,
                null(),
            );
            LocalFree(acl.cast());
            error
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        Ok(())
    }

    /// The process's user, as its token reports it.
    pub struct CurrentUser(Vec<u64>);

    impl CurrentUser {
        pub fn new() -> io::Result<CurrentUser> {
            let mut token: HANDLE = null_mut();
            // SAFETY: the token handle is closed below.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0;
            // SAFETY: the first call only reports the size needed; the
            // second writes at most `len` bytes into a buffer of that size,
            // aligned for TOKEN_USER.
            let result = unsafe {
                GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
                let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
                let ok = GetTokenInformation(
                    token,
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    len,
                    &mut len,
                );
                let error = io::Error::last_os_error();
                CloseHandle(token);
                if ok == 0 { Err(error) } else { Ok(buffer) }
            };
            result.map(CurrentUser)
        }

        /// The user's SID, valid while `self` is.
        pub fn sid(&self) -> PSID {
            // SAFETY: the buffer holds the TOKEN_USER that `new` read.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }
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

#[cfg(all(test, windows))]
mod tests {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    use windows_sys::core::PWSTR;

    use super::windows::CurrentUser;
    use super::*;

    /// Takes a string Windows allocated with LocalAlloc.
    unsafe fn take(text: PWSTR) -> String {
        // SAFETY: `text` is a NUL-terminated string from Windows.
        unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let owned = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            owned
        }
    }

    /// The file's access list in SDDL, such as `D:PAI(A;;FA;;;S-1-5-...)`.
    fn dacl(path: &Path) -> String {
        let file = File::open(path).unwrap();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the handle is open with READ_CONTROL, and `sddl` frees
        // the descriptor.
        unsafe {
            let error = GetSecurityInfo(
                file.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            );
            assert_eq!(error, 0, "GetSecurityInfo");
            sddl(descriptor)
        }
    }

    /// `text` as Windows writes it back, which abbreviates well-known SIDs,
    /// such as `LA` for the built-in administrator.
    fn normalised(text: &str) -> String {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated, and `sddl` frees the
        // descriptor.
        unsafe {
            let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            );
            assert_ne!(
                ok, 0,
                "ConvertStringSecurityDescriptorToSecurityDescriptorW"
            );
            sddl(descriptor)
        }
    }

    /// The access list of `descriptor` in SDDL, freeing `descriptor`.
    unsafe fn sddl(descriptor: PSECURITY_DESCRIPTOR) -> String {
        let mut text: PWSTR = null_mut();
        // SAFETY: `descriptor` is a valid descriptor from Windows, and
        // `take` frees the string.
        unsafe {
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                null_mut(),
            );
            LocalFree(descriptor);
            assert_ne!(
                ok, 0,
                "ConvertSecurityDescriptorToStringSecurityDescriptorW"
            );
            take(text)
        }
    }

    fn user_sid() -> String {
        let user = CurrentUser::new().unwrap();
        let mut text: PWSTR = null_mut();
        // SAFETY: `user` keeps the SID alive, and `take` frees the string.
        unsafe {
            assert_ne!(ConvertSidToStringSidW(user.sid(), &mut text), 0);
            take(text)
        }
    }

    #[test]
    fn the_file_grants_the_current_user_alone_and_inherits_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret/token");

        write(&path, b"fake-token-1").unwrap();

        // P: protected, so nothing is inherited. AI: SetSecurityInfo marks
        // the list as having taken part in inheritance. One entry: full
        // access for the current user.
        assert_eq!(
            dacl(&path),
            normalised(&format!("D:PAI(A;;FA;;;{})", user_sid()))
        );
        assert_eq!(read(&path).unwrap().as_deref(), Some(&b"fake-token-1"[..]));
    }
}
