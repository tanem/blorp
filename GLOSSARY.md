# blorp

A terminal UI for working through a Fastmail inbox quickly. This file is the glossary for the project's language.

## Language

**Queue**:
The threads matching the current filter, Inbox by default, in the order the user works through them. When more threads match than the queue holds, it holds the first of them in that order.
_Avoid_: List, feed, timeline, mailbox (for the queue)

**Filter**:
A typed expression that selects the queue: `in:`, `from:`, `to:`, `subject:`, `is:`, `has:` tokens and bare text, all of which must match, any of which can be negated. Without `in:` it applies to the Inbox.
_Avoid_: Search, query

**Queue mailbox**:
The mailbox the filter's `in:` names, Inbox when it names none, or every mailbox except Trash, Junk, Drafts, Sent, and the snooze mailbox for `in:any`. Move actions touch only a thread's messages in the queue mailbox.
_Avoid_: Current folder, scope

**View**:
A named, pinned filter and sort order shown in the tabline and switched with Tab.
_Avoid_: Tab, folder, saved search

**Selection**:
The threads an action applies to: the visual range if one is open, else the marked rows, else the count of rows from the cursor.
_Avoid_: Highlighted, checked

**Thread**:
A conversation as Fastmail groups it: the unit that every action applies to.
_Avoid_: Message (when the whole conversation is meant), email, conversation

**Triage**:
Working through the queue by giving each thread one action until the queue is empty.
_Avoid_: Processing, sorting, cleaning

**Action**:
One of the fixed set of things the user can do to the selection with a single key: archive, snooze, flag, delete, mark read or unread, move, mark spam, open in browser.
_Avoid_: Operation

**Command**:
An action or setting typed at the `:` prompt by name, with arguments, such as `:move Travel`.
_Avoid_: Ex command, colon command

**Snooze**:
Removing a thread from the queue until a chosen time, when it returns to the queue.
_Avoid_: Defer, remind, postpone

**Snooze mailbox**:
The `Snoozed (blorp)` mailbox that holds snoozed threads until they return. It is not Fastmail's own snoozed mailbox.
_Avoid_: Snoozed folder, snooze folder

**Return**:
A snoozed thread coming back to the queue, at its chosen time or early when a new email arrives in the thread.
_Avoid_: Unsnooze (for the automatic case), wake, resurface

**Undo**:
Reversing the most recent action so every thread it touched returns to the state it had before it.
_Avoid_: Revert, rollback

**Redo**:
Reapplying the most recently undone action.

**Agenda**:
A read-only view of upcoming calendar events shown beside the queue. Out of scope for v1.
_Avoid_: Calendar (for the pane), schedule (for the pane; schedule is the snooze record)

**Setup**:
The plain-terminal flow that takes an API token, checks it against Fastmail, and stores it. Runs on first launch or on request.
_Avoid_: Login, onboarding, auth

**Schedule**:
The record of every snoozed thread, where its messages came from, when it was snoozed, and when it returns.
_Avoid_: Snooze list, timers

**Reconcile**:
Bringing the queue back in line with Fastmail: triggered by a push event, once a minute, or by `R`.
_Avoid_: Sync (for the act), refresh (for the automatic case), poll

**Disconnected**:
The state blorp is in after a request to Fastmail has failed despite retries, until any request succeeds again. Actions are refused while disconnected; nothing is queued for later.
_Avoid_: Offline, down, degraded

**Dry run**:
A mode in which actions are logged instead of sent to Fastmail.
