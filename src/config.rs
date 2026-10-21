//! Local file paths, the config file and the views it holds, and the
//! settings tests give through the environment.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::Sort;
use crate::mail::SNOOZE_MAILBOX;

/// The words after `:view` that stand for its subcommands, and so cannot
/// be view names.
pub const RESERVED_VIEW_NAMES: [&str; 2] = ["save", "delete"];

/// The config file's name in [`config_dir`].
pub const FILE_NAME: &str = "config.toml";

/// blorp's config directory: `$XDG_CONFIG_HOME/blorp` when that is an
/// absolute path, else `$HOME/.config/blorp`. `env` looks up an
/// environment variable.
pub fn config_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = match env("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(env("HOME").filter(|home| !home.is_empty())?).join(".config"),
    };
    Some(base.join("blorp"))
}

/// blorp's state directory: `$XDG_STATE_HOME/blorp` when that is an
/// absolute path, else `$HOME/.local/state/blorp`. `env` looks up an
/// environment variable.
pub fn state_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = match env("XDG_STATE_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(env("HOME").filter(|home| !home.is_empty())?).join(".local/state"),
    };
    Some(base.join("blorp"))
}

/// A named filter and sort, shown in the tabline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    /// The name the tabline shows and `:view` switches by.
    pub name: String,
    /// The filter text, trimmed.
    pub filter: String,
    #[serde(default, skip_serializing_if = "is_newest")]
    pub sort: Sort,
}

/// Whether `sort` is the default, which the file leaves out.
fn is_newest(sort: &Sort) -> bool {
    *sort == Sort::Newest
}

/// The views there are until the config file lists its own: Inbox,
/// Unread, Flagged, and Snoozed.
pub fn default_views() -> Vec<View> {
    let view = |name: &str, filter: &str| View {
        name: name.into(),
        filter: filter.into(),
        sort: Sort::Newest,
    };
    vec![
        view("Inbox", ""),
        view("Unread", "is:unread"),
        view("Flagged", "is:flagged"),
        view("Snoozed", &format!("in:\"{SNOOZE_MAILBOX}\"")),
    ]
}

/// The config file's contents. Without a `views` key, the views are the
/// defaults.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    views: Option<Vec<View>>,
}

/// Why the config file could not be read.
#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    /// The file is not TOML, or not the shape of a config file. The
    /// message names the line.
    Parse(toml::de::Error),
    /// A view is named `save` or `delete`; holds the name.
    Reserved(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(error) => write!(f, "{error}"),
            LoadError::Parse(error) => write!(f, "{}", error.to_string().trim_end()),
            LoadError::Reserved(name) => write!(f, "view name \"{name}\" is reserved"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The views in the config file at `path`, or the defaults when there is
/// no file or it has no `views` key. A view named `save` or `delete` is an
/// error.
pub fn load(path: &Path) -> Result<Vec<View>, LoadError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(default_views()),
        Err(error) => return Err(LoadError::Io(error)),
    };
    let config: Config = toml::from_str(&text).map_err(LoadError::Parse)?;
    let views = config.views.unwrap_or_else(default_views);
    if let Some(view) = views
        .iter()
        .find(|v| RESERVED_VIEW_NAMES.contains(&v.name.as_str()))
    {
        return Err(LoadError::Reserved(view.name.clone()));
    }
    Ok(views)
}

/// The config file holding `views`, every one of them, so that from then
/// on the file is the whole list. No views is an empty `views` list, not
/// the defaults.
pub fn to_toml(views: &[View]) -> String {
    if views.is_empty() {
        return "views = []\n".into();
    }
    let config = Config {
        views: Some(views.to_vec()),
    };
    toml::to_string(&config).expect("views serialize")
}

/// Replaces the config file at `path` with `contents`, creating its
/// directory if it is missing, through a temp file and rename beside the
/// file a symlink at `path` points to, so the symlink stays.
pub fn save(path: &Path, contents: &str) -> io::Result<()> {
    let path = match fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(error) => return Err(error),
    };
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other(format!("{} names no file", path.display())))?;
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let written = fs::File::create(&temp)
        .and_then(|mut file| {
            file.write_all(contents.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

/// What a debug build reads from the environment so tests can run the
/// binary against a local server with no keychain.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DebugOverrides {
    /// `BLORP_SESSION_URL`, in place of Fastmail's session URL.
    pub session_url: Option<String>,
    /// `BLORP_TOKEN_STORE`, a file to hold the token in place of the
    /// keychain.
    pub token_store: Option<PathBuf>,
}

/// The overrides `env` sets, or none when `debug_build` is false, so a
/// release build always talks to Fastmail and the keychain. Empty values
/// count as unset.
pub fn debug_overrides(
    debug_build: bool,
    env: impl Fn(&str) -> Option<OsString>,
) -> DebugOverrides {
    if !debug_build {
        return DebugOverrides::default();
    }
    let var = |name| env(name).filter(|value| !value.is_empty());
    DebugOverrides {
        session_url: var("BLORP_SESSION_URL").map(|url| url.to_string_lossy().into_owned()),
        token_store: var("BLORP_TOKEN_STORE").map(PathBuf::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn a_debug_build_reads_the_session_url_and_token_store() {
        let vars = [
            ("BLORP_SESSION_URL", "http://127.0.0.1:4000/jmap/session"),
            ("BLORP_TOKEN_STORE", "/tmp/t/secret/token"),
        ];

        assert_eq!(
            debug_overrides(true, env(&vars)),
            DebugOverrides {
                session_url: Some("http://127.0.0.1:4000/jmap/session".into()),
                token_store: Some(PathBuf::from("/tmp/t/secret/token")),
            }
        );
    }

    #[test]
    fn a_release_build_ignores_the_session_url_and_token_store() {
        let vars = [
            ("BLORP_SESSION_URL", "http://127.0.0.1:4000/jmap/session"),
            ("BLORP_TOKEN_STORE", "/tmp/t/secret/token"),
        ];

        assert_eq!(
            debug_overrides(false, env(&vars)),
            DebugOverrides::default()
        );
    }

    #[test]
    fn empty_overrides_are_ignored() {
        let vars = [("BLORP_SESSION_URL", ""), ("BLORP_TOKEN_STORE", "")];

        assert_eq!(debug_overrides(true, env(&vars)), DebugOverrides::default());
    }

    #[test]
    fn state_dir_honours_xdg_state_home() {
        let vars = [("XDG_STATE_HOME", "/var/state"), ("HOME", "/Users/kim")];

        assert_eq!(
            state_dir(env(&vars)),
            Some(PathBuf::from("/var/state/blorp"))
        );
    }

    #[test]
    fn state_dir_falls_back_to_home() {
        let vars = [("HOME", "/Users/kim")];

        assert_eq!(
            state_dir(env(&vars)),
            Some(PathBuf::from("/Users/kim/.local/state/blorp"))
        );
    }

    #[test]
    fn state_dir_ignores_an_empty_or_relative_xdg_state_home() {
        for value in ["", "state"] {
            let vars = [("XDG_STATE_HOME", value), ("HOME", "/Users/kim")];

            assert_eq!(
                state_dir(env(&vars)),
                Some(PathBuf::from("/Users/kim/.local/state/blorp")),
                "{value:?}"
            );
        }
    }

    #[test]
    fn config_dir_honours_xdg_config_home() {
        let vars = [("XDG_CONFIG_HOME", "/etc/kim"), ("HOME", "/Users/kim")];

        assert_eq!(
            config_dir(env(&vars)),
            Some(PathBuf::from("/etc/kim/blorp"))
        );
    }

    #[test]
    fn config_dir_falls_back_to_home_and_ignores_a_relative_xdg_config_home() {
        for vars in [
            &[("HOME", "/Users/kim")][..],
            &[("XDG_CONFIG_HOME", "config"), ("HOME", "/Users/kim")],
            &[("XDG_CONFIG_HOME", ""), ("HOME", "/Users/kim")],
        ] {
            assert_eq!(
                config_dir(env(vars)),
                Some(PathBuf::from("/Users/kim/.config/blorp")),
                "{vars:?}"
            );
        }
        assert_eq!(config_dir(env(&[])), None);
    }

    #[test]
    fn state_dir_needs_a_home_without_xdg_state_home() {
        assert_eq!(state_dir(env(&[])), None);
    }
}
