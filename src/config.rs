//! Local file paths, and the settings tests give through the environment.

use std::ffi::OsString;
use std::path::PathBuf;

/// blorp's state directory: `$XDG_STATE_HOME/blorp` when that is an
/// absolute path, else `$HOME/.local/state/blorp`. `env` looks up an
/// environment variable.
#[cfg(not(windows))]
pub fn state_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = match env("XDG_STATE_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(env("HOME").filter(|home| !home.is_empty())?).join(".local/state"),
    };
    Some(base.join("blorp"))
}

/// blorp's state directory: `%LOCALAPPDATA%\blorp` when that is an absolute
/// path. `env` looks up an environment variable.
#[cfg(windows)]
pub fn state_dir(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = PathBuf::from(env("LOCALAPPDATA")?);
    base.is_absolute().then(|| base.join("blorp"))
}

/// What a debug build reads from the environment so tests can run the
/// binary against a local server with no keyring.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DebugOverrides {
    /// `BLORP_SESSION_URL`, in place of Fastmail's session URL.
    pub session_url: Option<String>,
    /// `BLORP_TOKEN_STORE`, a file to hold the token in place of the
    /// keyring.
    pub token_store: Option<PathBuf>,
}

/// The overrides `env` sets, or none when `debug_build` is false, so a
/// release build always talks to Fastmail and the keyring. Empty values
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

    #[cfg(not(windows))]
    #[test]
    fn state_dir_honours_xdg_state_home() {
        let vars = [("XDG_STATE_HOME", "/var/state"), ("HOME", "/Users/kim")];

        assert_eq!(
            state_dir(env(&vars)),
            Some(PathBuf::from("/var/state/blorp"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn state_dir_falls_back_to_home() {
        let vars = [("HOME", "/Users/kim")];

        assert_eq!(
            state_dir(env(&vars)),
            Some(PathBuf::from("/Users/kim/.local/state/blorp"))
        );
    }

    #[cfg(not(windows))]
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

    #[cfg(not(windows))]
    #[test]
    fn state_dir_needs_a_home_without_xdg_state_home() {
        assert_eq!(state_dir(env(&[])), None);
    }

    #[cfg(windows)]
    #[test]
    fn state_dir_is_under_local_app_data() {
        let vars = [("LOCALAPPDATA", r"C:\Users\kim\AppData\Local")];

        assert_eq!(
            state_dir(env(&vars)),
            Some(PathBuf::from(r"C:\Users\kim\AppData\Local\blorp"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn state_dir_needs_an_absolute_local_app_data() {
        for vars in [
            &[][..],
            &[("LOCALAPPDATA", "")],
            &[("LOCALAPPDATA", "Local")],
        ] {
            assert_eq!(state_dir(env(vars)), None, "{vars:?}");
        }
    }
}
