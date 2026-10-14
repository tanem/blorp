# Triage queue prototype

Throwaway. Second round for wayfinder ticket 06 after the reframe: blorp is a triage queue in the Octobox sense, not a mail reader. No reading pane; `o` opens the thread in Fastmail. Fake data, no network, nothing persisted. Do not grow this into the app.

Run:

```
cargo run --manifest-path prototypes/triage-queue/Cargo.toml
```

Four layout variants, switched with `[` and `]` or `F1` to `F4`. The yellow bar at the bottom is the prototype switcher, not part of the design. Press `?` for the keymap. `--snapshot` prints every screen as text.

- D, tabs: pinned views with counts across the top, one wide table, agenda as a single line.
- E, sidebar: views and mailboxes with counts on the left, table in the centre, agenda strip on the right.
- F, grouped: rows grouped by sender; an action on a group header applies to the whole group; Enter collapses a group.
- G, calendar: the agenda drives extra snooze presets ("after Standup") and the peek pane is on by default.

Shared in every variant: `/` types a filter (`in:`, `from:`, `is:unread`, `is:flagged`, `is:list`, `is:invite`, `has:attachment`, or any text), `Tab` and `1`-`9` switch pinned view, `x` selects a row, `X` selects everything shown, actions apply to the selection or the row under the cursor, `u` undoes a whole batch, `y` and `n` accept or decline an invitation row, `a` toggles the agenda, `p` toggles an 8-line peek pane. Changing the view or filter clears the selection so an action never hits rows that are off screen.
