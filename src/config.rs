//! Local file paths.

use std::ffi::OsString;
use std::path::PathBuf;

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
    fn state_dir_needs_a_home_without_xdg_state_home() {
        assert_eq!(state_dir(env(&[])), None);
    }
}
