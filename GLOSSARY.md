# blorp

A terminal UI for a Fastmail account's mailboxes over JMAP. This file is the glossary for the project's language.

## Language

**Queue**:
The threads of the mailbox shown, newest first, in the order the user works through them. When the mailbox holds more threads than the queue holds, it holds the newest of them.
_Avoid_: List, feed, timeline, mailbox (for the queue)

**Mailbox shown**:
The mailbox whose threads the queue holds: the Inbox at launch, and after that the one last switched to with `m`. Its path is on the statusline.
_Avoid_: Current folder, view

**Mailbox list**:
The list `m` opens over the queue: every mailbox by path, filtered by the text typed, for switching the mailbox shown.
_Avoid_: Picker, folder list, menu

**Thread**:
A conversation as Fastmail groups it: one row of the queue.
_Avoid_: Message (when the whole conversation is meant), email, conversation

**Unread**:
A thread any email of which Fastmail has not marked seen, in whatever mailbox that email is. Shown with a dot on its row. Marking a thread read marks every email of it, in every mailbox.
_Avoid_: New, unseen

**Unread only**:
A setting of the queue, off at launch and toggled with `u`, under which the queue holds only the unread threads of the mailbox shown. It stays on across a switch of the mailbox shown.
_Avoid_: View, filter, unread mode

**Search**:
The text typed after `/` that narrows the queue to those of its threads whose row's sender or subject contains it, ignoring case; the mailbox's threads beyond the queue are not searched. Empty at launch, and cleared with Esc or by a switch of the mailbox shown.
_Avoid_: Filter, find, query

**Action**:
A change blorp makes to the account's mail on a key, such as archiving a thread. It shows in the queue at once and is sent through the outbox.
_Avoid_: Command, operation, mutation

**Outbox**:
The actions typed and not yet answered by Fastmail, sent one at a time in the order typed.
_Avoid_: Job queue, send queue

**Setup**:
The plain-terminal flow that takes an API token, checks it against Fastmail, and stores it. Runs on first launch or on request.
_Avoid_: Login, onboarding, auth

**Keyring**:
The platform's store for secrets, where setup keeps the API token: the login keychain on macOS, Secret Service on Linux and other Unix systems, Credential Manager on Windows.
_Avoid_: Keychain (except for macOS's), vault, secret store

**Reconcile**:
Bringing the queue back in line with Fastmail: triggered by a push event or once a minute.
_Avoid_: Sync (for the act), refresh (for the automatic case), poll

**Disconnected**:
The state blorp is in after a request to Fastmail has failed despite retries, until any request succeeds again.
_Avoid_: Offline, down, degraded

**Probe**:
A reconcile made while disconnected, tried once and spaced out by a growing wait, to find out whether Fastmail can be reached again.
_Avoid_: Ping, health check, heartbeat
