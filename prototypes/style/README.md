# Style prototype

Throwaway. Third round for wayfinder ticket 06: look and feel, plus nvim-style mechanics. The structure from round two is assumed (triage queue, filter, pinned views, bulk selection, no reading pane). Fake data, no network, nothing persisted. Do not grow this into the app.

Run:

```
cargo run --manifest-path prototypes/style/Cargo.toml
```

Four skins, switched with `[` and `]` or `F1` to `F4`. The yellow bar at the bottom is the prototype switcher, not part of the design. Press `?` for the keys. `--snapshot` prints every screen as text.

- H, C64: blue screen with a light-blue border, everything uppercase, a `READY.` prompt above the input line, the queue as a directory listing, the header counts threads free.
- I, Arcade: black with yellow and red, a HUD with score, hi-score, stage (the pinned view), threads left, and undo slots as lives. Each action scores; consecutive actions of the same kind build a combo. Inbox zero shows STAGE CLEAR.
- J, Nvim: no borders, a tabline of pinned views, relative line numbers in the gutter, a lualine-style statusline with a mode block, and a `:` command line. Visual line mode shows `-- VISUAL LINE --`.
- K, Minimal: one column per thread (subject, then sender and age dimmed), one accent colour, a breadcrumb for view and filter, and two nano-style key-hint lines at the bottom.

Mechanics shared by every skin, borrowed from nvim: a count prefix (`3j`, `3e`, `5G`), `V` for visual line selection extended with `j` and `k`, `.` to repeat the last action, `x` to mark rows and `X` to mark everything shown, `/` to filter, and `:` for commands (`archive`, `delete`, `flag`, `read`, `undo`, `move <mailbox>`, `snooze <preset>`, `view <name>`, `filter <query>`, `sort oldest`, `q`). Actions apply to the visual range, else the marks, else the count from the cursor. `u` undoes a whole batch. Digits are counts here, so pinned views switch with `Tab` or `:view`.
