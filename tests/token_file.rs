//! The token file that replaces the keychain in debug builds when
//! `BLORP_TOKEN_STORE` is set.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use blorp::cli::{TokenFile, TokenStore};
use secrecy::{ExposeSecret, SecretString};

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

/// A path in a directory blorp creates, since a temp directory is 0755.
fn token_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("secret/token")
}

fn stored(file: &TokenFile) -> Option<String> {
    file.get()
        .unwrap()
        .map(|token| token.expose_secret().to_string())
}

#[test]
fn no_file_means_no_token() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(stored(&TokenFile::new(dir.path().join("token"))), None);
}

#[test]
fn the_token_is_written_0600_in_a_new_0700_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_path(&dir);
    let file = TokenFile::new(&path);

    file.set(&SecretString::from("fake-token-1")).unwrap();

    assert_eq!(stored(&file).as_deref(), Some("fake-token-1"));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[test]
fn setting_replaces_the_token_and_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_path(&dir);
    let file = TokenFile::new(&path);

    file.set(&SecretString::from("fake-token-1")).unwrap();
    file.set(&SecretString::from("fake-token-2")).unwrap();

    assert_eq!(stored(&file).as_deref(), Some("fake-token-2"));
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["token"]);
}

#[test]
fn delete_removes_the_token_and_succeeds_when_there_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let file = TokenFile::new(token_path(&dir));
    file.set(&SecretString::from("fake-token-1")).unwrap();

    assert!(file.delete().unwrap());
    assert_eq!(stored(&file), None);
    assert!(!file.delete().unwrap());
}

#[test]
fn a_symlink_is_refused_and_its_target_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    fs::write(&target, "kept").unwrap();
    let path = dir.path().join("token");
    symlink(&target, &path).unwrap();
    let file = TokenFile::new(&path);

    assert!(file.get().is_err(), "get");
    assert!(
        file.set(&SecretString::from("fake-token-1")).is_err(),
        "set"
    );
    assert!(file.delete().is_err(), "delete");
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept");
    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
}

#[test]
fn a_directory_others_can_open_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let open = dir.path().join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    let file = TokenFile::new(open.join("token"));

    let error = file.set(&SecretString::from("fake-token-1")).unwrap_err();

    assert!(error.to_string().contains("0700"), "{error}");
    assert!(!open.join("token").exists());
}
