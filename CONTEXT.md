# sift

A terminal UI for working through a Fastmail inbox quickly. This file is the glossary for the project's language.

## Language

**Queue**:
The set of threads currently in the Inbox mailbox, in the order the user works through them.
_Avoid_: List, feed, timeline

**Thread**:
A conversation as Fastmail groups it: the unit that every action applies to.
_Avoid_: Message (when the whole conversation is meant), email, conversation

**Triage**:
Working through the queue by giving each thread one action until the queue is empty.
_Avoid_: Processing, sorting, cleaning

**Action**:
One of the fixed set of things the user can do to a thread with a single key: archive, snooze, flag, delete, mark read or unread, move, mark spam, open in browser.
_Avoid_: Command, operation

**Snooze**:
Removing a thread from the queue until a chosen time, when it returns to the queue.
_Avoid_: Defer, remind, postpone

**Undo**:
Reversing the most recent action so the thread returns to the state it had before it.

**Agenda**:
A read-only view of upcoming calendar events shown beside the queue.
_Avoid_: Calendar (for the pane), schedule

**Dry run**:
A mode in which actions are logged instead of sent to Fastmail.
