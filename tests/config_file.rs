//! The config file: views as `[[views]]` entries, the defaults when there
//! are none, and a file that does not parse.

use std::fs;

use blorp::app::Sort;
use blorp::config::{self, View};

fn view(name: &str, filter: &str, sort: Sort) -> View {
    View {
        name: name.into(),
        filter: filter.into(),
        sort,
    }
}

fn defaults() -> Vec<View> {
    vec![
        view("Inbox", "", Sort::Newest),
        view("Unread", "is:unread", Sort::Newest),
        view("Flagged", "is:flagged", Sort::Newest),
        view("Snoozed", "in:\"Snoozed (blorp)\"", Sort::Newest),
    ]
}

#[test]
fn an_absent_file_means_the_default_views_and_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blorp").join(config::FILE_NAME);

    assert_eq!(config::load(&path).unwrap(), defaults());
    assert!(!path.exists());
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn a_file_with_no_views_key_means_the_default_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);
    fs::write(&path, "# nothing here yet\n").unwrap();

    assert_eq!(config::load(&path).unwrap(), defaults());
}

#[test]
fn views_are_read_in_order_with_sort_defaulting_to_newest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);
    fs::write(
        &path,
        r#"[[views]]
name = "Travel"
filter = "in:Travel"
sort = "oldest"

[[views]]
name = "Inbox"
filter = ""
"#,
    )
    .unwrap();

    assert_eq!(
        config::load(&path).unwrap(),
        [
            view("Travel", "in:Travel", Sort::Oldest),
            view("Inbox", "", Sort::Newest),
        ]
    );
}

#[test]
fn an_empty_views_list_is_no_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);
    fs::write(&path, "views = []\n").unwrap();

    assert_eq!(config::load(&path).unwrap(), []);
}

#[test]
fn a_file_that_does_not_parse_is_an_error_naming_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);
    fs::write(&path, "[[views]]\nname = \"Travel\"\nfilter = in:Travel\n").unwrap();

    let error = config::load(&path).unwrap_err().to_string();

    assert!(error.contains("line 3"), "{error}");
}

#[test]
fn an_unknown_sort_is_an_error_naming_the_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);
    fs::write(
        &path,
        "[[views]]\nname = \"Travel\"\nfilter = \"in:Travel\"\nsort = \"sideways\"\n",
    )
    .unwrap();

    let error = config::load(&path).unwrap_err().to_string();

    assert!(error.contains("line 4"), "{error}");
}

#[test]
fn views_are_written_as_views_entries_with_sort_only_when_oldest() {
    let written = config::to_toml(&[
        view("Inbox", "", Sort::Newest),
        view("Travel", "in:Travel", Sort::Oldest),
    ]);

    assert_eq!(
        written,
        r#"[[views]]
name = "Inbox"
filter = ""

[[views]]
name = "Travel"
filter = "in:Travel"
sort = "oldest"
"#
    );
}

#[test]
fn no_views_are_written_as_an_empty_list_so_the_defaults_stay_away() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(config::FILE_NAME);

    config::save(&path, &config::to_toml(&[])).unwrap();

    assert_eq!(config::load(&path).unwrap(), []);
}

#[test]
fn save_creates_the_directory_and_load_reads_back_what_it_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("config")
        .join("blorp")
        .join(config::FILE_NAME);
    let views = vec![
        view("Snoozed", "in:\"Snoozed (blorp)\"", Sort::Oldest),
        view("Work stuff", "in:Work from:\"Jane Doe\"", Sort::Newest),
    ];

    config::save(&path, &config::to_toml(&views)).unwrap();

    assert_eq!(config::load(&path).unwrap(), views);
    let names: Vec<_> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [config::FILE_NAME], "no temp file is left behind");
}

#[test]
fn a_write_that_fails_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blorp");
    fs::write(&blocker, "a file where the directory goes").unwrap();

    let result = config::save(&blocker.join(config::FILE_NAME), "views = []\n");

    assert!(result.is_err());
}

#[test]
fn save_through_a_symlink_writes_the_file_it_points_to_and_keeps_the_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles").join("blorp.toml");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "views = []\n").unwrap();
    let path = dir.path().join(config::FILE_NAME);
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let views = vec![view("Travel", "in:Travel", Sort::Newest)];

    config::save(&path, &config::to_toml(&views)).unwrap();

    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    assert_eq!(config::load(&target).unwrap(), views);
}

#[test]
fn a_view_named_save_or_delete_is_an_error() {
    for name in ["save", "delete"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(config::FILE_NAME);
        let text = format!(
            "[[views]]\nname = \"Inbox\"\nfilter = \"\"\n\n[[views]]\nname = \"{name}\"\nfilter = \"\"\n"
        );
        fs::write(&path, text).unwrap();

        let error = config::load(&path).unwrap_err().to_string();

        assert_eq!(error, format!("view name \"{name}\" is reserved"));
    }
}
