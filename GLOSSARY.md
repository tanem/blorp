# blorp

A terminal UI for working through a Fastmail inbox quickly. This file is the glossary for the project's language.

## Language

**Queue**:
The Inbox's threads, newest first, in the order the user works through them. When the Inbox holds more threads than the queue holds, it holds the newest of them.
_Avoid_: List, feed, timeline, mailbox (for the queue)

**Thread**:
A conversation as Fastmail groups it: one row of the queue.
_Avoid_: Message (when the whole conversation is meant), email, conversation

**Triage**:
Working through the queue by giving each thread one action until the queue is empty.
_Avoid_: Processing, sorting, cleaning

**Agenda**:
A read-only view of upcoming calendar events shown beside the queue. Out of scope for v1.
_Avoid_: Calendar (for the pane), schedule (for the pane)

**Setup**:
The plain-terminal flow that takes an API token, checks it against Fastmail, and stores it. Runs on first launch or on request.
_Avoid_: Login, onboarding, auth

**Reconcile**:
Bringing the queue back in line with Fastmail: triggered by a push event or once a minute.
_Avoid_: Sync (for the act), refresh (for the automatic case), poll

**Disconnected**:
The state blorp is in after a request to Fastmail has failed despite retries, until any request succeeds again.
_Avoid_: Offline, down, degraded

**Probe**:
A reconcile made while disconnected, tried once and spaced out by a growing wait, to find out whether Fastmail can be reached again.
_Avoid_: Ping, health check, heartbeat
