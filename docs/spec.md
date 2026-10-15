# blorp v1 specification

blorp is a terminal application that triages a single Fastmail account's mailboxes over JMAP. This document is the v1 specification: everything decided while charting the effort, organised by section rather than by the ticket that decided it, and restated in full because the map directory it was assembled from is machine-local scratch. Each section links the tickets it draws on, under `.scratch/triage-tui/issues/` on the machine that charted the map, and the research or prototype branch that holds the reasoning. Where two tickets amended each other, only the final rule appears here. Contradictions and gaps found while assembling are listed under Open points at the end, not resolved in place. `CONTEXT.md` at the repository root is the glossary, and this document uses its terms.

## Purpose and scope

blorp is a triage tool in the Octobox sense, not a mail reader. It shows a queue of threads, one per row, and the user gives each thread one action with a single key until the queue is empty. Reading happens in the browser: `o` opens the thread in Fastmail's web UI. There is no reading pane, no peek pane, and no compose, reply, or forward.

v1 is for Tane alone, with nothing hardcoded to the account and distribution deferred. It is a Rust application built on ratatui, developed against the real inbox with a dry-run mode, in a public repository under the MIT licence from the first commit. It handles one account. The calendar agenda is out of scope.

Existing clients were surveyed on 2026-10-14 as sources of ideas only: aerc (Go), meli (Rust), and himalaya 2.x (Rust CLI, TUI unreleased) speak JMAP natively; none does Fastmail snooze or calendar. The name `blorp` was free on crates.io and checked for trademark collisions on the same day.

Sources: [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), prototypes on branch [prototype/layout-keymap](https://github.com/tanem/blorp/tree/prototype/layout-keymap/prototypes).

## Auth and setup

### Token storage

The Fastmail API token is a secret throughout. It lives only in a `secrecy::SecretString` with a single `expose_secret()` call site, which builds the `Authorization` header through `reqwest::bearer_auth` so that `Debug` prints `Sensitive`. Nothing logs, formats, serialises, or panics with it, and errors never carry it. The token is never a command-line argument and never written to the config file.

The token is stored in the macOS login keychain as a generic-password item with service `blorp` and account the fixed string `fastmail`, via `keyring-core` with `apple-native-keyring-store` (`keychain` feature only); direct `security-framework` use is an acceptable equivalent. A fixed account string lets the keychain alone start blorp, so losing the config file never forces a token re-paste. The Fastmail username is displayed during setup and stored nowhere. A rebuilt ad-hoc-signed development binary triggers one keychain prompt; that is expected.

`BLORP_API_TOKEN` in the environment is used in place of the keychain when set, for CI and scripted dry runs only.

The token's Fastmail scope is Email only: not read-only, and no Email submission. At startup blorp checks that the session advertises `urn:ietf:params:jmap:mail`.

Any future secret-bearing file is mode 0600 in a 0700 directory, written through a temp file and rename, with symlinks refused.

### Command shape

`blorp` starts the TUI. `blorp setup` runs or reruns token setup and overwrites an existing keychain item. `blorp logout` deletes the keychain item (`NoEntry` counts as success) and leaves the config file and the snooze schedule alone, so pending returns survive a re-setup. Flags on the bare command are `--dry-run`, `--version`, and `--help`. There is no launch flag for a mailbox or filter.

`blorp setup` ignores `BLORP_API_TOKEN` and still prompts and writes the keychain. `blorp logout` warns, if the variable is set, that the environment token will still be used.

### First run

When `blorp` finds no token (keychain `NoEntry` and `BLORP_API_TOKEN` unset) it runs setup on plain stdin and stdout before the TUI starts. Setup prints where to create the token (Fastmail Settings, Privacy & Security, Connected apps & API tokens, scope Email only), prompts with echo off or reads stdin when stdin is not a terminal, fetches `/jmap/session`, checks that it advertises `urn:ietf:params:jmap:mail`, writes the keychain item, prints the username the session reports, and starts the TUI. Setup is never shown inside the ratatui UI, and the keychain item is only overwritten on a successful setup.

### Account

blorp uses `primaryAccounts["urn:ietf:params:jmap:mail"]` from the session on every launch, ignores every other entry in `accounts`, and caches no account id.

### Token rejected

On any HTTP 401, including from the push stream or a poll mid-session, blorp restores the terminal, prints the fixed message "Fastmail rejected the API token. It may have been removed in Settings. Run `blorp setup` to store a new one." and exits with status 1. A 403, or a session without the mail capability, exits the same way with a message naming the Email scope. Setup is never launched from inside the TUI; the user runs `blorp setup` by hand. Revocation is a manual action in Fastmail Settings.

Sources: [Token storage and secret handling standard](../.scratch/triage-tui/issues/03-token-storage.md) with findings on [research/token-storage](https://github.com/tanem/blorp/blob/research/token-storage/docs/research/token-storage.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).

## The queue and filter language

### Queue

The queue is the set of threads matching the current filter. Launch opens Inbox. Other mailboxes are reached in-app with `/in:<mailbox>`, `:filter in:<mailbox>`, or a view. Any mailbox except Drafts and Sent can be a queue. Trash and Junk open as queues so a misfiled thread can be moved back out, but `d` and `!` are disabled there because they would destroy permanently. System mailboxes are identified by their Fastmail role (inbox, archive, trash, junk, snoozed, drafts, sent).

The unit of the queue is the thread. A thread is in the queue when any of its emails matches the filter; it is unread when any email lacks `$seen` and flagged when any email has `$flagged`.

### Execution

Every filter is one server-side `Email/query` with `collapseThreads: true`, followed by `Thread/get` and `Email/get` chained by back-reference in the same request, so the rows have every email of each thread with its `mailboxIds` and keywords. Nothing is filtered client-side; the in-memory queue is the query result. The server sorts by `receivedAt`; the only client-side step is the `returnedAt` reorder described under Snooze.

### Tokens

A filter is space-separated tokens, all of which must match: `in:<mailbox>`, `from:<text>`, `to:<text>`, `subject:<text>`, `is:unread`, `is:read`, `is:flagged`, `is:list` (a `List-Id` header exists), `has:attachment`, and bare text. Bare text maps to the `text` condition, so it matches from, to, cc, bcc, subject, and body as Fastmail indexes them, the same threads the web search box finds. `-token` negates one token. Double quotes wrap a value with spaces: `in:"Snoozed (blorp)"`, `from:"Jane Doe"`. There is no `OR` and no grouping. `is:invite` does not exist: it is not expressible in `Email/query`, and calendar is out of scope.

### Queue mailbox

A filter with no `in:` token is scoped to Inbox. `in:any` means every mailbox except Trash, Junk, Drafts, and Sent. The queue mailbox is the mailbox the filter's `in:` names, Inbox when it names none, or that set for `in:any`; move actions touch only a thread's messages in the queue mailbox.

### Mailbox names

`in:` matches the mailbox name case-insensitively and exactly. Nested mailboxes are written as a path: `in:Work/Receipts`. The role names `inbox`, `archive`, `trash`, `junk` match the mailbox carrying that role whatever it is called. A name matching nothing or more than one mailbox is an error. The same rules resolve the argument of `:move`.

### Empty and invalid

`/` then Enter on an empty prompt means the Inbox default, as does `:filter` with no argument. An unknown token key, unknown `is:` value, unbalanced quote, or unknown mailbox leaves the queue unchanged and shows the error in the status line. `/` opens the prompt pre-filled with the current filter text for editing; Esc restores the previous queue.

### Sort

Every queue sorts newest first by default, with `t` and `:sort` toggling to oldest first; `:sort newest` and `:sort oldest` set it directly. The Snoozed queue sorts by return time, soonest first, with `t` reversing it. Every other queue sorts by max(`receivedAt`, `returnedAt`), which the server sort cannot express, so the `returnedAt` reorder is applied client-side after the query.

Sources: [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md), [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), [Thread membership across mailboxes](../.scratch/triage-tui/issues/13-thread-membership.md).

## Views

A view is a name, a filter, and a sort (`newest` or `oldest`, default `newest`). Switching to a view sets both. `t` and `:sort` change the current queue only and never alter the saved view.

The default set, in tabline order, is `Inbox` (empty filter), `Unread` (`is:unread`), `Flagged` (`is:flagged`), and `Snoozed` (`in:"Snoozed (blorp)"`).

`:view <name>` switches. `:view save <name>` pins the current filter and sort, overwriting a view of the same name. `:view delete <name>` removes one. `save` and `delete` are reserved and cannot be view names. Tabline order is save order; reordering is by editing the config file, with no in-app reorder in v1. `Tab` and `Shift-Tab` switch views.

Views persist as `[[views]]` entries in `~/.config/blorp/config.toml` with `name`, `filter`, and optional `sort`. An absent file or absent `views` key means the default set. The first `:view save` or `:view delete` writes the full list; from then on the file is the whole truth and the defaults can be deleted or renamed.

When the current filter matches no saved view, no view is marked active and a transient trailing tab shows the filter text. `Tab` from there goes to the first view.

Sources: [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).

## Keymap and command line

### Look

The screen is the nvim skin from the prototypes: no borders, a tabline of views across the top, relative line numbers, one thread per row (sender column, subject, age, flags, whole-thread message count), a lualine-style statusline with a mode block, and a bottom line that is the filter prompt, the command line, or the status message. Terminal colours. In the Snoozed queue the age column shows the return time.

The statusline shows the mode block, queue name and filter, position and count, the dry-run badge, the disconnected badge, the undo depth as a small counter (`u3`, hidden at zero), and the last action message.

### Modes

Normal mode is the default and single keys act immediately. There are no persistent modes. Counts, visual line, the filter prompt, and the command line are transient modes that return to normal on Enter or Esc.

### Keys

Keys act on the selection: the visual range if one is open, else the marked rows, else `count` rows from the cursor.

- Move: `j` `k`, `g` `G`, count prefix (`3j`, `12G`). Digits are always counts, never view switches.
- Actions: `e` archive, `z` snooze picker, `f` flag, `d` delete, `r` read/unread, `m` move picker, `!` spam, `o` open in browser, `u` undo, `Ctrl-r` redo.
- Selection: `x` mark or unmark the row, `X` clear marks, `V` visual line extended with `j` and `k` (Esc leaves), `.` repeat the last action on the current target.
- Queue: `/` filter, `Tab` and `Shift-Tab` switch view, `t` toggle newest and oldest, `R` refresh.
- Other: `:` command line, `D` dry-run toggle, `?` help overlay listing keys and commands, `q` quit.

After an action the cursor stays on the same row so the next thread slides into place. Keys during the initial load act on an empty queue and do nothing.

### Pickers

The snooze picker (`z`) offers the four fixed presets, each with a letter hotkey, plus `j` and `k`, Enter, and Esc. The move picker (`m`) filters mailbox names as the user types, moves with Ctrl-n and Ctrl-p or the arrow keys, and takes Enter or Esc. Both act on the whole selection and report messages such as "Snoozed 3 threads".

### Command line

`:` opens a one-line prompt; Enter runs the command and returns to normal mode. A command acts on the same selection as a key: the visual range open when `:` is pressed, else the marked rows, else `count` rows from the cursor, with the count typed before the colon (`3:archive`). There is no range syntax inside the prompt.

The vocabulary is `:archive` `:delete` `:spam` `:flag` `:read` `:unread` `:open` `:refresh` `:move <mailbox>` `:snooze <time>` `:unsnooze` `:undo [count]` `:redo` `:view [save|delete] <name>` `:filter [query]` `:sort [newest|oldest]` `:dry [off]` `:help` `:quit`. `:help` opens the same overlay as `?`. `:read` and `:unread` are explicit while the `r` key toggles. There is no `:set`, no `:mark`, `:unmark`, or `:clear`: marks are keys only, and typing `:set` gives "Unknown command: set".

Aliases: `:e` archive, `:d` delete, `:f` flag, `:r` read, `:o` open, `:m` move, `:z` snooze, `:uz` unsnooze, `:u` undo, `:v` view, `:h` help, `:q` quit. A command name is lowercase and either exact or a listed alias; there is no unique-prefix matching.

Arguments: `:move` takes the rest of the line as the mailbox name, without quotes, resolved by the `in:` rules; unknown or ambiguous names are errors and the command never falls back to the picker. `:filter` with no argument resets to the Inbox default. `:sort` with no argument toggles like `t`. `:view` with no argument is a usage error. `:snooze` argument forms are under Snooze.

Completion: Tab completes the word under the cursor, Shift-Tab cycles back, and a line above the prompt lists the candidates. The first word completes command names and aliases. `:move`, and `in:` inside `:filter`, complete mailbox paths, matching on the path segment being typed so `Work/Re` completes to `Work/Receipts`. `:view` completes `save`, `delete`, and view names. `:snooze` completes the four presets and weekday names. `:sort` completes `newest` and `oldest`. `:filter` also completes token keys and `is:` values.

History: Up and Down walk this session's command history filtered by the text already typed. The `/` prompt has its own history. Neither is persisted; both are unbounded within the session. A failed command stays in history so Up recalls it to fix.

Editing keys: Left, Right, Home, End, Ctrl-a, Ctrl-e, Backspace, Ctrl-w delete word, Ctrl-u clear line, Esc cancel, Enter run.

Errors close the prompt and show one of these in the status line: `Unknown command: foo`; `Usage: :move <mailbox>` for missing or extra arguments; `No mailbox matches "Trav"`; `Mailbox name "Receipts" is ambiguous: Work/Receipts, Home/Receipts`; `Snooze time not understood: "next tuesday"`; `No view named "Later"`; `Nothing selected`.

Sources: [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md) with prototypes on [prototype/layout-keymap](https://github.com/tanem/blorp/tree/prototype/layout-keymap/prototypes), [Command line vocabulary and completion](../.scratch/triage-tui/issues/12-command-line.md), [Undo semantics](../.scratch/triage-tui/issues/08-undo-semantics.md).

## Actions and thread membership

The action set is archive, snooze, flag, delete, mark read or unread, move, mark spam, open in browser, undo, and redo. Every action except open is an `Email/set` patch on `mailboxIds` and `keywords` (`$seen`, `$flagged`); undo is an `Email/get` followed by the inverse patch. Delete and spam are moves to the Trash and Junk mailboxes.

### Move actions (`e`, `d`, `!`, `m`, `z`)

Each moves only the thread's messages that are in the queue mailbox; messages elsewhere are untouched. Archive from Inbox moves the Inbox messages and leaves a Travel message in Travel. Messages in Trash, Junk, Drafts, or Sent are never moved by any action: archive does not pull a Sent reply into Archive and delete does not touch a Junk message. In a Trash or Junk queue, `m` and `e` move only the messages in Trash or Junk, so rescuing a misfiled thread restores just the misfiled part; `d` and `!` are disabled there. In an `in:any` queue the queue mailbox is the whole triageable set, so these actions sweep every message outside the four excluded mailboxes.

`e` always moves to the Archive mailbox whatever queue the thread is in. In Archive it does nothing and says so; a no-op pushes nothing onto the undo stack.

### Keyword actions (`f`, `r`)

Apply to every message of the thread, in every mailbox including Trash, Junk, Drafts, and Sent, so a thread marked read has no unread email left anywhere. `r` on a thread with any unread email marks all read, otherwise all unread. `f` on a thread with any flagged email unflags all, otherwise flags all. `:read` and `:unread` set the state explicitly.

### Row

The count shown is the whole thread's message count, which stays stable as messages are archived and refiled. There is no marker for a thread partly outside the queue mailbox, in any queue including `in:any`.

### Reference behaviour

Fastmail's web client archives and moves only the current folder's messages, deletes every message in every folder by default, marks read and unread across the whole conversation, flags only the representative email but unflags the whole thread, and shows the whole-thread count. blorp matches it for archive, move, read, unread, and the count, and diverges on delete (queue mailbox, not all folders) and flag (whole thread, not one email), so that each action family has one rule and matches snooze.

Sources: [Thread membership across mailboxes](../.scratch/triage-tui/issues/13-thread-membership.md), [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md).

## Bulk selection

The selection is the visual range if one is open, else the marked rows, else the count of rows from the cursor, defaulting to the row under it. `x` marks or unmarks a row, `X` clears marks, and `V` opens a visual range extended with `j` and `k` and left with Esc. A bulk action is one `Email/set`, chunked at `maxObjectsInSet` if ever needed, and one undo entry. `.` repeats the last action on the current target, each repeat its own undo entry.

Selection clears when the filter or view changes. After an action the marks on the threads it removed are dropped and the cursor stays on its row. After a reconcile, marks and a visual range on threads that left the queue are dropped and the others survive. Undo does not restore marks or a visual range.

Sources: [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), [Undo semantics](../.scratch/triage-tui/issues/08-undo-semantics.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).

## Snooze

Fastmail does not offer JMAP snooze to API-token clients. Checked 2026-10-14 with a read-only Email-scope token: the session advertises only `urn:ietf:params:jmap:core` and `urn:ietf:params:jmap:mail`, `Email/get` with the `snoozed` property is rejected with `invalidArguments`, and adding the snooze capability to `using` yields a response with no `methodResponses` (an unknown-capability error per RFC 8620, exact body unverified). A mailbox with role `snoozed` exists and Fastmail's web client uses it. blorp therefore emulates snooze.

### Mailbox

blorp creates a mailbox named `Snoozed (blorp)` with no role on first snooze and recreates it if it is missing. Fastmail's own `snoozed`-role mailbox is not used: a third-party report says Fastmail bounces moves into it, and Fastmail's web UI would show the threads as snoozed with no return time.

### Schedule

The schedule is a local file, one entry per snoozed thread: the thread id, each moved email id with the mailbox it came from, the return time, and a `returnedAt` set when the thread returns. It is the one piece of local mail-related state; see Local files for its path and format. A thread found in the snooze mailbox with no entry is treated as due now. In dry run nothing is written, the schedule included.

### Which messages

Snoozing moves the thread's messages that are in the queue mailbox to the snooze mailbox; messages in other mailboxes are untouched. In an `in:any` queue `z` moves every message outside Trash, Junk, Drafts, and Sent, each recorded with its own origin. Return moves exactly those messages back to the mailbox each came from, so a thread snoozed from `in:Travel` returns to Travel. Mailboxes are recorded by id so a rename in Fastmail does not break the return.

### Return

blorp checks for due entries at launch, on every reconcile, and on `R`. There is no launchd job or daemon, so a thread due while blorp is closed returns at the next launch. On return the thread goes back to its origin mailbox marked unread, flag untouched. `receivedAt` is immutable in JMAP, so the entry records `returnedAt` and the queue sorts by max(`receivedAt`, `returnedAt`), which puts the returned thread at the top. The entry is dropped once the thread is next actioned or leaves the queue. Automatic return is not undoable.

A new reply lands in Inbox and the thread reappears in the queue. When blorp sees a thread with messages both in the queue and in the snooze mailbox, it cancels the snooze and moves the older messages back, so the thread is whole again. This matches Fastmail's own behaviour.

### Presets

Presets use the local time zone. Later today is now plus 3 hours rounded up to the hour, or tomorrow morning if that is past 20:00. Tomorrow morning is 08:00 the next day. This weekend is Saturday 08:00, or next Saturday if today is Saturday or Sunday. Next week is Monday 08:00. The hours are fixed in v1.

### Custom times

`:snooze` accepts the four preset names `later`, `tomorrow`, `weekend`, `next`, and: durations such as `30m`, `2h`, `3d`, `1w`; a clock time such as `17:30` (today, or tomorrow if past); a weekday such as `mon`, optionally with a time such as `mon 9`; an ISO date such as `2026-11-02`, optionally with a time. Anything else is the error `Snooze time not understood`. There is no natural-language parsing. The `z` picker stays presets only.

### The snooze mailbox as a queue

`in:"Snoozed (blorp)"` opens it like any mailbox, and the default `Snoozed` view does so. Rows show the return time in the age column and sort by it, soonest first. There, `z` reschedules and `:unsnooze` (alias `:uz`) returns the selection now. `e`, `d`, `!`, and `m` work as usual and cancel the entry. There is no single key for unsnooze.

Sources: [Verify whether Fastmail offers JMAP snooze to API-token clients](../.scratch/triage-tui/issues/01-verify-snooze-capability.md), [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).

## Undo and redo

JMAP has no undo, so blorp records what it changes. At action time it records, for each affected email, its prior `mailboxIds` and `$seen` and `$flagged` keywords, plus the thread's schedule entry if it has one, and pushes one entry per action. The snapshot is taken from the emails already in memory. Undo applies the snapshot back, so a mixed selection restores exactly and a thread spanning mailboxes goes back where each message was.

Undoable: archive, delete, move, flag, read and unread, spam, snooze, `:unsnooze`, and `z` reschedule in the snooze mailbox. Not on the stack: `o`, refresh, filter and view changes, the sort toggle, the dry-run toggle, marks, automatic snooze return, and no-ops such as `e` in Archive.

A bulk action is one entry; each `.` repeat is its own entry. `u` undoes one, `3u` undoes three, and `:undo` takes an optional count. `Ctrl-r` and `:redo` replay the action's patch. History is linear: a new action after an undo discards the redo entries. The stack is unbounded within a session, cleared on quit, has no time expiry, and is shared across filters and views because it belongs to the account, not the queue. Toggling dry run in either direction clears it.

Before applying an undo blorp runs `Email/get` on the affected ids. An email whose `mailboxIds` and keywords still match what the action left is restored; one changed since (moved in the web client, destroyed) is skipped. A new reply is a different email and does not count. The entry pops either way and the message reports the partial result.

After undo the cursor jumps to the first restored thread if it is in the current queue. Marks and the visual range are not restored. A restored thread outside the current filter is still restored and the message names the mailbox it went to.

Status line messages: "Undone: archive 3 threads", "Undone: archive 2 of 3 threads, 1 changed elsewhere", "Undone: archive 1 thread (in Travel)", "Redone: ...", "Nothing to undo", "Nothing to redo".

Snooze interactions: undo of a snooze moves the messages back and deletes the schedule entry. Undo of `:unsnooze` re-snoozes at the original return time. Undo of a reschedule restores the previous time. Undo of `e`, `d`, `m`, or `!` in the snooze mailbox restores the entry they cancelled. Undo of an action on a returned thread restores its entry with `returnedAt`, so it returns to the top of the queue.

Dry-run actions change the in-memory queue and push entries; undo and redo are logged like any action and the changed-elsewhere check is skipped because nothing was sent.

Sources: [Undo semantics](../.scratch/triage-tui/issues/08-undo-semantics.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).

## Sync model and reconcile

### Stateless

There is no on-disk mail cache. The queue is fetched per launch and held in memory. The schedule, the config file, and the per-session log are the only local files, and none of them caches mail.

### Wake-up signal

Fastmail serves JMAP push over EventSource at the session's `eventSourceUrl` and accepts API tokens there. blorp opens one stream with `types=Email,Mailbox`, `closeafter=no`, and `ping=300`, with exponential reconnect backoff (1 s doubling to a 60 s cap with jitter, unbounded attempts) and a ping watchdog that treats silence for `ping` plus 30 s as a dead stream. Known quirks: the event data carries an extra `"type": "connect"` key beside `@type: StateChange`, the first event on connect carries the current state strings, and `ping` is clamped to at most 300 s.

Whenever the stream is down, a 60 s timer polls `Email/changes` and `Mailbox/changes` with the last known state strings as the fallback. `R` is the manual refresh. WebSocket push, `PushSubscription`, and `Email/queryChanges` are not used: Fastmail does not advertise WebSocket push and has not implemented `PushSubscription`, and Cyrus supports `Email/queryChanges` only for plain `inMailbox` queries.

Push and poll are invisible outside the `sync` task, which emits a single message, `Wake`, and nothing else. A dead stream with a working poll still counts as connected.

### Reconcile

On a `Wake`, a poll tick, or `R`, blorp re-runs the current query and diffs the result against the queue, rather than patching the list from `Email/changes`. If a reconcile is already in flight a dirty flag is set and it re-runs when the current one lands. A reconcile also runs the due-snooze check.

The result replaces the queue's row set: new threads enter at their sorted position, gone threads leave, changed threads update in place. The cursor stays on the same thread if it is still present, otherwise at the same index clamped to the new length. Marks and a visual range on threads that left are dropped; others survive. The undo stack is untouched.

A reconcile result is applied only if the outbox was idle from the moment the query was issued to the moment its result arrived; otherwise it is discarded and re-run once the outbox drains, so a server snapshot taken before a move landed never resurrects a thread just archived.

Fastmail's documented limits are the advertised capability values (`maxConcurrentRequests` 10, `maxCallsInRequest` 50, `maxObjectsInGet` 4096) and 500 logins per 10 minutes. The design holds at most one outbox job, one queue query, and one stream open at a time.

Sources: [Change detection while running: push or poll](../.scratch/triage-tui/issues/04-live-sync.md) with findings on [research/live-sync](https://github.com/tanem/blorp/blob/research/live-sync/docs/research/live-sync.md), [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).

## Error and offline behaviour

### Failure classes

Every failed request is sorted into one of three classes before `update` sees it.

- Transient: DNS, connect, reset, and timeout errors; HTTP 5xx; HTTP 429; the JMAP request-level `limit` error; the method errors `serverUnavailable` and `serverFail`; the `rateLimit` SetError. A `Retry-After` header is honoured up to 60 s; otherwise the backoff below applies.
- Fatal: 401, 403, and a session without the mail capability. The terminal is restored, the fixed message printed, exit 1.
- Bug: everything else. HTTP 4xx other than 401, 403, and 429; `notJSON`, `notRequest`, `unknownCapability`, `unknownMethod`, `invalidArguments`, `invalidResultReference`, `forbidden`, `accountNotFound`, `accountReadOnly`, `requestTooLarge`, `serverPartialFail`, `unsupportedFilter`, `unsupportedSort`; and a response that does not parse. Never retried. The status line shows "Fastmail error: <type>" and the full response, minus headers, is logged at error level. A bug failure on an action is handled like an exhausted transient one.

### Retry and timeouts

A transient failure is retried three times with 1, 2, and 4 s delays plus up to 500 ms of jitter, for every request kind. Timeouts are 10 s to connect and 30 s for a whole query, get, or set request. The EventSource stream has no request timeout beyond its ping watchdog.

### Unreachable at launch

The session fetch and `Mailbox/get` run on the plain terminal with the retry schedule above. On exhaustion blorp prints "Could not reach Fastmail: <error>" and exits 1. There is no empty TUI that keeps retrying: with no cache there is nothing to show.

### Connection state

`App` holds a connection state, `connected` or `disconnected`, with no intermediate state. Any request that exhausts its retries moves it to `disconnected`; any request that succeeds moves it back. While an attempt is being retried the status line shows "Retrying Fastmail (2/3)" over the spinner. On entering `disconnected` the status line says "Disconnected from Fastmail" and a persistent `disconnected` badge appears in the statusline's right segment, beside the dry-run badge. On return the badge clears and the status line says "Reconnected".

### Queries and reconcile

A filter, view, sort, or refresh query that fails leaves the queue and the current filter as they were and reports in the status line. A reconcile that fails is dropped silently apart from the connection state change. While `disconnected`, a reconcile runs as a probe with its own backoff, 5 s doubling to a 60 s cap, in place of the normal tick; `R` and a `Wake` run a probe at once and reset that backoff. The first probe that succeeds applies its result as an ordinary reconcile, so the queue catches up in the same step that clears the badge. The first event after a stream reconnect is a `Wake` like any other.

### Actions while disconnected

Every action and every command that would send to Fastmail is refused: the status line says "Disconnected, action not sent", the queue is untouched, no undo entry is pushed, and no job enters the outbox. Navigation, marks, visual range, `o`, dry-run toggling, and `R` still work. Filter and view changes are allowed to try, since they are the cheapest probe, and fail per the rule above. Nothing accumulates across launches.

### Actions that fail in flight

An `Email/set` that fails transiently is retried on the same schedule inside its outbox job, so later jobs wait behind it and the server-side order is kept. The patches are idempotent (`mailboxIds/<id>` and `keywords/<k>` set to `true` or `null`), so a retry after an ambiguous timeout is safe. On exhaustion, or on a bug failure, the status line reports, the undo entry is popped, the schedule change is reverted, the queue mutation is rolled back, and a reconcile is issued, which also fixes the queue if the ambiguous attempt did land. An outbox failure that exhausts its retries moves the state to `disconnected`; the jobs behind it fail immediately, each rolled back the same way, and the outbox is empty when the probe loop takes over. `notUpdated` on some ids of a bulk action is not a transport failure: the status line reports it, the undo entry is popped, the schedule change is reverted, and a reconcile is issued.

### Rate limiting

A 429, `limit`, or `rateLimit` response is transient and honours `Retry-After`. `maxConcurrentRequests` is never approached, so a `limit` error is a signal to slow down rather than a bug.

### Local I/O failures

A schedule write that fails cancels the snooze before the `Email/set` is sent: it is rolled back and reported, because a thread in the snooze mailbox with no entry would return at the next launch. A config write that fails on `:view save` or `:view delete` is reported, and the in-memory views stay changed for the session. A log write failure is ignored; the non-blocking appender drops lines.

### Quit with a job in flight

`q` and `:quit` wait up to 3 s for the outbox to drain. If a job is still unconfirmed after that, blorp restores the terminal and prints "1 action was not confirmed by Fastmail; check the Inbox." before exiting 0. In dry run and while `disconnected` the outbox is always empty, so quit is immediate.

Sources: [Error and offline behaviour](../.scratch/triage-tui/issues/15-error-and-offline-behaviour.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).

## Dry run

Dry run is a mode in which actions are logged instead of sent to Fastmail. It is enabled by `--dry-run` at launch, which prints one plain line before the TUI starts, or toggled with `D` and `:dry [off]`. It is never remembered in config: every launch starts live unless the flag is given.

It is surfaced by the statusline badge and a `[dry]` prefix on every action message. Dry-run actions change the in-memory queue, push undo entries, and are logged at info level as the action plus thread ids; the JMAP effect is replaced by a log effect. Nothing is written to the schedule. Toggling dry run in either direction clears the undo stack and says so, so real and simulated entries never share a stack.

Reconcile is suspended in dry run so push and poll do not overwrite the simulated queue; `Wake` is ignored. `R` resets the queue to server state and clears the undo stack. The disconnected probe loop still runs so the badge stays truthful, but its results are discarded.

Sources: [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md), [Undo semantics](../.scratch/triage-tui/issues/08-undo-semantics.md), [Error and offline behaviour](../.scratch/triage-tui/issues/15-error-and-offline-behaviour.md).

## Local files

### Config

`~/.config/blorp/config.toml`, honouring `$XDG_CONFIG_HOME`, TOML via serde. It holds user preferences only: `[[views]]` entries are the first and so far only content. No account section, no token, no dry-run default, no `:set` keys. An absent file means defaults and blorp does not create it until something is saved. A file that fails to parse stops the launch with the parse error and line number. It is read once in `main` and written synchronously on `:view save` and `:view delete`.

### Snooze schedule

`~/.local/state/blorp/snooze.json`, honouring `$XDG_STATE_HOME`, JSON via serde. The whole file is rewritten on every change through a temp file and rename, mode 0600 in a 0700 directory, synchronously by the executor at the moment the local state changes and before the matching `Email/set` is sent. Shape: a `version` of 1 and a `threads` map keyed by thread id, each entry holding `emails` (a map from email id to origin mailbox id), `returnAt` (RFC 3339 with the local offset at snooze time), and `returnedAt` (null until the thread returns). A file that fails to parse is renamed to `snooze.json.broken-<timestamp>`, the status line reports it, and threads in the snooze mailbox with no entry return now.

### Log

`~/.local/state/blorp/blorp.log`, truncated at each launch, default level warn, raised with `BLORP_LOG` in `RUST_LOG` syntax. Logging is `tracing` with `tracing-appender`'s non-blocking file writer, initialised in `main`. Nothing in it ever carries the token or raw headers. No rotation in v1.

### Keychain

Service `blorp`, account `fastmail`, generic password, as under Auth and setup.

Sources: [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md), [Token storage and secret handling standard](../.scratch/triage-tui/issues/03-token-storage.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).

## Architecture

### JMAP client

blorp hand-rolls a small JMAP module on reqwest and serde, with eventsource-stream for the push stream, using io-jmap's `rfc8620` module as the reference shape. It uses six methods, one download URL template, and one SSE stream; every Fastmail detail (the download host, the bare `eventSourceUrl`, the quirks above) is a line in blorp's own code. jmap-client 0.4.3 was rejected: its closed `using` enum and header catch-all need a fork for any extension, and it is broken against Fastmail (open issues #32 and #36). io-jmap 0.6.0 handles extensions through its generic layer but is blocking, no_std, single-connection, and four months old; it is worth revisiting if calendars or contacts over JMAP are added.

### Runtime

One tokio runtime. One task, the state task, owns the whole `App` state and is the only code that reads or writes it. It runs `tokio::select!` over three sources: crossterm's `EventStream` (keys, resize), an unbounded mpsc channel of messages from background tasks (query results, action results, sync wake-ups, errors), and a 250 ms tick that runs only while a request is in flight, for the spinner. It redraws once after draining everything that is ready. No locks, no shared state, nothing blocking on the state task. Every JMAP call is a spawned task owning a clone of the client that reports back on the channel.

### Pure core, effects out

The state task's heart is `update(&mut App, Msg) -> Vec<Effect>`: pure, synchronous, no I/O. Effects are what the executor around it carries out: send a JMAP request tagged with a generation, write the schedule, write config, open a URL, log a line, exit with a fatal message. The executor spawns each effect and feeds its outcome back as a message. Dry run is one branch in `update`. This is recorded as ADR 0001 in `docs/adr/`. `App` holds a `now` field that the state task sets from a clock before each `update` call. The production clock is the system clock; executor tests use a settable clock alongside tokio's paused time.

### Queries and cancellation

Every queue query (filter, view, sort, refresh, reconcile) carries a generation number bumped on each change. The result message carries its generation and `update` drops a stale one. The executor also aborts the superseded request through a `CancellationToken`, so a fast `/` edit never stacks requests. One query is one JMAP request: `Email/query` with `collapseThreads`, `Thread/get`, and `Email/get` chained by back-reference.

### Optimistic actions

An action mutates the queue at once (thread removed, cursor moved, marks on it dropped), pushes the undo entry from the emails already in memory, applies any schedule change, and then emits the `Email/set` effect. On a response with `notUpdated` ids or a transport error the status line reports it, the undo entry is popped, the schedule change is reverted, and a reconcile query is issued. The user never waits for the server during triage.

### Outbox

Actions, undo, redo, snooze returns, and the schedule-cancelling moves go through one FIFO outbox task that sends one job at a time, so the server sees them in the order the user acted. A job may be several calls: undo is `Email/get` then `Email/set`; the launch-time snooze return is one job per due thread. Reconcile queries run concurrently with the outbox under the discard rule described under Sync model and reconcile.

### Sync task

One `sync` task owns the EventSource connection, its reconnect backoff and ping watchdog, and the 60 s fallback timer. It emits one message, `Wake`, and nothing else; `update` decides what to do with it.

### Side I/O

The keychain read is synchronous in `main` before the runtime starts; the token goes into the JMAP client as a `SecretString` and nothing else holds it. The schedule is written synchronously by the executor (a few KB and an atomic rename, so no `spawn_blocking`). Config is read once in `main` and written synchronously. `o` opens the thread through the `open` crate as an effect.

### Startup

`main` does, in order and synchronously on the plain terminal: parse args, load config, read the keychain (run setup when absent), fetch `/jmap/session` and check the mail capability, `Mailbox/get`. Only then does it enter raw mode and start the TUI. The initial queue query and the due-snooze return are dispatched as ordinary messages, so the loading state is the same code path as every later filter change. A 401 or 403 before the TUI prints and exits with no terminal to restore; after it, the same condition is an `Effect::Fatal` that restores the terminal, prints the fixed message, and exits 1.

### Modules

A lib crate (`src/lib.rs`) with a thin `src/main.rs`, so integration tests under `tests/` reach the core.

- `jmap`: transport and wire types. A `Jmap` trait with two adapters, reqwest for production and an in-memory fake holding a small mail store for tests. The seam is at the method level (`query`, `get`, `set`, `changes`, `events`), not raw HTTP.
- `mail`: the in-memory snapshot of fetched mailboxes, threads, and emails, with role lookup and the mailbox name resolution rules. Pure.
- `filter`: filter text to AST to `Email/query` JSON, resolving mailbox names through `mail`. Pure.
- `queue`: rows, sort, cursor, marks, visual range, selection resolution, and the reconcile diff. Pure.
- `snooze`: the schedule, presets, time parsing, due computation, origins; takes an injected clock. Pure apart from one persist function.
- `undo`: stack, snapshot, patch, linear redo. Pure.
- `app`: the `App` state, `Msg`, `Effect`, and `update`. Turns an action on a selection into queue mutation, undo entry, schedule change, and effects. This is the deep module: the whole triage behaviour sits behind `update`.
- `ui`: `view(&App, &mut Frame)` and key event to `Msg`, including the command line, completion, and history. Pure given `App`.
- `config`: paths, `config.toml` load and save, with injected base directories.
- `cli`: args, `setup`, `logout`, behind a token-store trait the keyring adapter implements.
- `sync`: the EventSource and poll task.
- The executor and the state task loop live in `main.rs` or a small `run` module. The run loop takes a generic stream of key events and a ratatui `Backend` so tests can run it headless.

### Crates

tokio (with `test-util` for tests), crossterm with `event-stream`, ratatui, reqwest, serde, eventsource-stream, tokio-util, tracing, tracing-appender, secrecy, keyring-core with apple-native-keyring-store, toml, clap, open; and for tests insta, wiremock, tempfile.

Sources: [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md), [Choose the JMAP client crate](../.scratch/triage-tui/issues/02-jmap-crate.md) with findings on [research/jmap-crate](https://github.com/tanem/blorp/blob/research/jmap-crate/docs/research/jmap-crate.md), [ADR 0001](adr/0001-pure-core-with-effects.md).

## Testing

### Rule

`cargo test` touches no network, no keychain, and nothing under the home directory. Every test runs on a fresh checkout with no secrets. The one exception is the live smoke test, which is ignored by default.

### Fixtures

All test data is synthetic, built in Rust by a store builder: mailboxes with roles and nesting, threads, and emails with from, to, subject, received date, keywords, mailbox ids, and an attachment flag. Addresses and names are invented. No response from the real account is ever captured into the repository, scrubbed or otherwise. The `jmap` module's serde types are tested against hand-written wire fixtures modelled on the RFC 8620 and 8621 examples and the Fastmail session shape; a Fastmail quirk found during development is added as a hand-written fixture.

### No recorded traffic

Nothing records or replays real JMAP sessions. The reqwest adapter is tested on its own against a local mock HTTP server (wiremock) with hand-written JSON responses: session fetch and the mail capability check, the request envelope and back-references, 429 with `Retry-After`, 5xx retried on the schedule, connect and request timeouts, 401 and 403 as fatal, and a non-JSON body as a bug-class failure. The EventSource parser is tested on a static byte stream and the reconnect backoff as a pure state machine.

### The fake

The in-memory `Jmap` adapter is a store, not a request matcher. It evaluates a subset of `Email/query`: `inMailbox`, `inMailboxOtherThan`, `hasKeyword`, `notKeyword`, `from`, `to`, `subject`, and `text` as case-insensitive substring matches, `hasAttachment`, AND, and NOT; sorts by received date in either direction; honours `collapseThreads`. It applies `Email/set` patches to its store and returns `notUpdated` for unknown ids. It supports failure injection (the next n calls fail with a given error or delay) and keeps a call log. Deviations from Fastmail's semantics are a known risk that the live smoke test and dry-run sessions exist to catch.

### `update` tests

Triage behaviour is tested at the `update` level, with no runtime: actions on a selection, counts, marks and visual range, undo and redo including the partial-restore path, snooze scheduling and return, early return on reply, the reconcile diff and cursor rule, optimistic rollback on `notUpdated` and on transport failure, the disconnected state and its refusals, dry run, the command line, completion, and history. A harness wraps `App`: it feeds key events through the `ui` keymap, then calls `update`, and exposes the queue rows, cursor, status line, undo depth, and the effects returned. The harness is also how snapshot states are built. A test sets the `now` field directly.

### Executor tests

Integration tests under `tests/` run the real state task loop and executor against the fake, with tokio's paused time so backoff and tick timers are instant, headless on `TestBackend` with a scripted key stream. They cover outbox FIFO ordering across actions, undo, and snooze returns; generation bump and abort of a superseded query; discarding a reconcile that arrived while the outbox was busy and re-running it; retry exhaustion moving to `disconnected`, the probe backoff, and the first success applying as a reconcile; the launch sequence from session fetch to initial query and due-snooze return; and quit draining the outbox within 3 s and printing the unconfirmed line otherwise.

### Rendering

`view` is snapshot-tested with insta on `TestBackend` at 80x24 and 120x40 over a fixed set of states: empty queue, loaded queue with marks and a visual range, relative numbers, the command line with a completion candidate line, the filter prompt, the snooze and move pickers, the retrying and disconnected statusline, the dry-run badge, and the transient ad hoc view tab. Snapshots live beside the `ui` module. CI runs insta in CI mode, so a new or changed snapshot fails the build; `cargo insta review` accepts changes locally.

### Filter

The parser and the `in:` name rules are table-driven unit tests over three columns: filter text, the AST, and the `Email/query` JSON produced, resolved against a small mail snapshot with nested mailboxes, mixed-case names, and role mailboxes. Error rows cover unknown tokens, unbalanced quotes, an unknown mailbox, and the reserved view names. No property-based tests.

### Files, setup, secrets

The `config` and `snooze` persistence code take injected base directories and are tested in a `tempfile` directory: atomic write and the 0600 mode, a broken schedule renamed aside, a config parse error stopping the launch, and the first `:view save` writing the full default list. `cli::setup` and `cli::logout` are tested with a fake token store and scripted input and captured output; the keyring adapter itself has no automated test. One test runs an action, a failed request, and a fatal 401 with a capturing tracing subscriber and asserts the token string appears nowhere in the captured output.

The fake adapter, the store builder, and the harness live in the lib crate as ordinary public modules, always compiled, with no cargo feature.

### Live smoke test

One test marked `#[ignore]` runs only under `cargo test -- --ignored` with `BLORP_API_TOKEN` set. It fetches the session, checks the mail capability, runs `Mailbox/get`, and runs one Inbox `Email/query` with `collapseThreads`, asserting on shapes only. It writes nothing, asserts on no content, and never runs in GitHub Actions.

### Not adopted

No coverage reporting or gate and no mutation testing; both are one-line additions later.

Sources: [Testing strategy](../.scratch/triage-tui/issues/16-testing-strategy.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).

## Repository and CI

The repository is github.com/tanem/blorp, public, MIT, configured on 2026-10-16 with `tanem/mt5-pnl-cli` as the reference.

- Merge: merge commits only; squash and rebase are off at the repository level and the `main` ruleset allows only `merge`. Delete branch on merge is on; auto-merge is off.
- Features: projects, wiki, and discussions off; issues on. Topics: email, fastmail, jmap, ratatui, rust, triage, tui.
- Ruleset `main` on `refs/heads/main`: no deletion, no force push, pull request required with zero approvals, extra approval required for unattributed changes. Bypass: repository Admin role, always, so Tane can push to `main` directly. Agent branches go through a pull request and merge with a merge commit.
- Security: Dependabot alerts and security updates, secret scanning, push protection, and private vulnerability reporting all on.
- Actions: enabled, limited to GitHub-owned and verified-creator actions, default `GITHUB_TOKEN` read-only, Actions cannot approve pull requests.

CI is one GitHub Actions workflow on `macos-latest`, triggered on pushes to `main` and on pull requests, running `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`. It uses the runner's preinstalled rustup and only GitHub-owned actions (`actions/checkout`, `actions/cache` for the cargo registry and target directory). No secrets. Once the workflow exists its job becomes the required status check in the `main` ruleset.

`.scratch/` is gitignored and machine-local. `CONTEXT.md` is the glossary and `docs/adr/` holds the architecture decision records.

Sources: [GitHub repository settings and branch protection](../.scratch/triage-tui/issues/10-github-repo-settings.md), [Testing strategy](../.scratch/triage-tui/issues/16-testing-strategy.md).

## Out of scope

- Compose, reply, forward.
- OAuth client registration and any distribution channel (Homebrew, crates.io, binaries).
- Multiple accounts.
- On-disk mail cache or offline mode, including offline queueing of actions across launches.
- Calendar agenda, calendar-driven snooze presets, and inline invitation answers. CalDAV needs a second credential and a crate; a later effort.
- Calendar event creation or editing.
- A background job or daemon that returns snoozed threads on time while blorp is closed. v1 returns them at the next launch.
- Reading mail in the terminal, including HTML-to-text rendering. The research on [Rendering HTML mail as text](../.scratch/triage-tui/issues/05-html-to-text.md), with findings on [research/html-to-text](https://github.com/tanem/blorp/blob/research/html-to-text/docs/research/html-to-text.md), recommends html2text 0.17.3 with the `css` feature and is kept for a later phase.
- `:set` keys and configurable preset hours.
- In-app reordering of views.
- Log rotation.

## Open points

Contradictions and gaps found while assembling, with the tickets they fall between. None is resolved here.

1. Dry-run logging is invisible by default. Dry-run actions are logged at info, but the log's default level is warn, so "logged instead of sent" produces nothing unless `BLORP_LOG` is raised. [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).
2. Snooze return in dry run. Reconcile is suspended in dry run and nothing is written to the schedule, but the due-snooze check at launch and on `R` is not stated for dry run: whether due threads are returned on the server, simulated in memory, or skipped. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).
3. Return destination for a thread with no schedule entry. A thread found in the snooze mailbox with no entry is due now, but its origin mailbox is unknown; the destination (Inbox is the obvious candidate) is not stated. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [First-run setup and dry-run flow](../.scratch/triage-tui/issues/09-setup-and-dry-run.md).
4. How the snooze mailbox is inspected. The due-snooze check needs to see every thread in `Snoozed (blorp)`, including ones with no entry, but the only query specified is the current filter's. Whether reconcile runs a second query against the snooze mailbox, or only due entries from the schedule are acted on, is not stated. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).
5. Early return on reply is only detected in the queue where the reply lands. A thread snoozed from Travel whose reply arrives in Inbox is only seen whole when the Inbox queue is open; in another queue the snooze stays. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md).
6. The default `Snoozed` view before the mailbox exists. The mailbox is created on first snooze, but the default view's filter names it, so on a fresh account `Tab` to Snoozed hits the "No mailbox matches" error. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md).
7. Mailbox list refresh mid-session. `Mailbox/get` runs once at startup and `Mailbox/changes` is part of the poll, but nothing states when the in-memory mailbox list (used by `in:`, `:move`, the picker, and the snooze mailbox lookup) is refetched after a mailbox is created or renamed elsewhere. [Change detection while running: push or poll](../.scratch/triage-tui/issues/04-live-sync.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).
8. The browser URL for `o`. The thread is opened in Fastmail's web UI through the `open` crate, but the URL shape for a thread is not recorded anywhere. [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).
9. `o` and `:open` on a multi-thread selection. Whether they open one tab per thread, only the cursor's thread, or refuse is not stated. [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), [Command line vocabulary and completion](../.scratch/triage-tui/issues/12-command-line.md).
10. The `.` repeat target. Ticket 06 says `.` repeats the last action "on the current target"; whether that means the cursor row only or the current selection as defined by marks and count is not stated. [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md), [Undo semantics](../.scratch/triage-tui/issues/08-undo-semantics.md).
11. Due-snooze check cadence with a healthy push stream. Ticket 07 checks for due entries on every reconcile tick, meaning the 60 s poll, but ticket 04 runs that poll only while the stream is down and ticket 14 runs the check on each reconcile, which a healthy stream triggers only on a server change. A thread due at 08:00 may therefore return only on the next unrelated `Wake` or on `R`. [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md), [Change detection while running: push or poll](../.scratch/triage-tui/issues/04-live-sync.md), [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md).
12. Ownership of the fallback timer while disconnected. Ticket 14 gives the `sync` task the 60 s fallback timer and has it emit only `Wake`; ticket 15 has `update` run a probe with its own 5 s to 60 s backoff in place of the normal tick, reset by any `Wake`. If `sync` keeps ticking while disconnected, every tick resets the probe backoff; if `update` replaces the tick, `sync` must stop its timer, which is not stated. Also unstated: whether the poll emits `Wake` on every tick or only when state strings change, and what it emits when `Email/changes` itself fails. [App architecture: runtime, event loop, and request flow](../.scratch/triage-tui/issues/14-app-architecture.md), [Error and offline behaviour](../.scratch/triage-tui/issues/15-error-and-offline-behaviour.md).
13. The `sort` field of the `Snoozed` view. A view's sort is `newest` or `oldest`, but the Snoozed queue sorts by return time, soonest first. What `sort = "newest"` means there, what `:sort oldest` does, and whether the return-time sort applies only to the exact filter `in:"Snoozed (blorp)"` or to any filter containing it, is not stated. [Filter language and pinned views](../.scratch/triage-tui/issues/11-filter-language-and-views.md), [Snooze design](../.scratch/triage-tui/issues/07-snooze-design.md).
14. Help overlay dismissal keys, and the exact statusline mode block texts, are implied by the prototypes but not recorded as decisions. [Layout and keymap prototype](../.scratch/triage-tui/issues/06-layout-keymap.md).
