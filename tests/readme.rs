//! The README's key table lists the key bindings in the code.

use blorp::keymap::BINDINGS;

const README: &str = include_str!("../README.md");

/// The key table's row for a count, which no binding stands for.
const COUNT: &str = "| A count, then `j` or `k` | Down or up that many rows |";

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

    let mut table = key_table();
    let count = table
        .iter()
        .position(|row| *row == COUNT)
        .expect("the key table has a row for a count");
    table.remove(count);

    assert_eq!(table, expected);
}
