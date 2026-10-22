//! The README's key table lists the key bindings in the code.

use blorp::keymap::BINDINGS;

const README: &str = include_str!("../README.md");

/// The body rows of the first table under the `## Keys` heading.
fn key_table() -> Vec<&'static str> {
    let section = README
        .split("\n## ")
        .find(|section| section.starts_with("Keys"))
        .expect("the README has a Keys section");
    section
        .lines()
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(str::trim_end)
        .collect()
}

#[test]
fn the_readme_key_table_lists_every_binding_in_order() {
    let expected: Vec<String> = BINDINGS
        .iter()
        .map(|binding| {
            let keys: Vec<String> = binding.keys.iter().map(|key| format!("`{key}`")).collect();
            format!("| {} | {} |", keys.join(", "), binding.help)
        })
        .collect();

    assert_eq!(key_table(), expected);
}
