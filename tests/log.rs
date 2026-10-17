//! Runs in its own process, since it installs the global subscriber.

use std::fs;

#[test]
fn the_log_is_truncated_at_launch_and_filtered_at_warn_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blorp").join("blorp.log");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "from the last launch\n").unwrap();

    let guard = blorp::log::init(&path, None).unwrap();
    tracing::info!("an info line");
    tracing::warn!("a warn line");
    tracing::info!(target: "blorp::dry_run", "a dry-run line");
    drop(guard);

    let log = fs::read_to_string(&path).unwrap();
    assert!(!log.contains("from the last launch"), "{log}");
    assert!(!log.contains("an info line"), "{log}");
    assert!(log.contains("a warn line"), "{log}");
    assert!(log.contains("a dry-run line"), "{log}");
}
