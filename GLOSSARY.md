# blorp

A terminal UI for a Fastmail account's Inbox over JMAP. This file is the glossary for the project's language.

## Language

**Queue**:
The Inbox's threads, newest first, in the order the user works through them. When the Inbox holds more threads than the queue holds, it holds the newest of them.
_Avoid_: List, feed, timeline, mailbox (for the queue)

**Thread**:
A conversation as Fastmail groups it: one row of the queue.
_Avoid_: Message (when the whole conversation is meant), email, conversation

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
