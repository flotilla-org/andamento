# Managed primary content

A workspace can be bound to a stable intent while the terminal content resolving
that intent changes. A project-role entity is one example; its entity identity
and action.primary.target remain stable across backing convoy attempts.

This first slice supports one command terminal in a host-owned primary slot.
It does not implement Flotilla role discovery, arbitrary content graphs, daemon
attachment replacement, or multiple managed slots.

## Producer facts

These optional facts opt an entity into reconciliation:

| Fact | Meaning |
| --- | --- |
| workspace.primary.state | ready or held |
| workspace.primary.target | Opaque identity of this resolved backing target; required when ready |
| action.primary.recipe | Command to launch for the current resolution; required when ready |
| checkout.path | Optional command working directory |

The resolved target must change when replacing the backing instance even when
the command text stays the same. It is distinct from action.primary.target,
which names the stable workspace intent. Changes to the command or cwd also
require reconciliation. Labels and unrelated metadata do not.

Held explicitly suspends replacement. Missing, expired, malformed or incomplete
resolution facts yield Unavailable. Both retain applied host content. Unavailable
is not deletion. Publish each resolution coherently, with consistent TTLs for
its state, target, recipe and cwd. Actual transport loss becomes unavailable when
those facts expire; this slice does not add a connection-status transport.

Resolution reads the metadata store independently of grouping, placement,
collapse and visibility. Closing a workspace removes its local binding; it does
not remove the producer's intent.

## Host interface

Sidebar.managed owns desired/applied state and monotonically allocated tokens.
Its plan operation takes workspace ID, intent entity and the host's actual
persisted descriptor. It returns Unavailable, Held, Current, Updating or Failed.
An Updating plan includes an owned descriptor and token. Repeated plans return
the same pending update until the binding or desired resolution changes.

Prepare the host resource without modifying existing content, validate the token
immediately before mutation, commit on the same owner thread as publication,
then acknowledge success. A stale result must never mutate the host first.
Close/reopen, entity rebinding, changed resolution and expiry invalidate pending
tokens. Failure retains applied content and suppresses automatic retries until
explicit retry or a changed desired descriptor. This avoids per-frame spawn
loops. Unchanged publication is not a retry request.

The additive C interface is andamento_content_plan/get/valid/complete/retry/
release. Plans own their borrowed strings and can outlive subsequent mutations,
but their token must still validate before committing. Topology observation
forgets closed workspace bindings. No existing ABI-2 structure changed.

Workspace presence and attachment currency are separate: a Live workspace can
have held, unavailable, failed or updating content. The existing snapshot Live
state alone is not evidence that its backing content is current.

## Evidence and limits

Core regression coverage includes changes during preparation, old completions,
failure/retry, held state, expiry/reconnect, unchanged metadata, hidden entities
and close/reopen. Wheelhouse's managed-content diagnostic exercises the typed
C path with real in-process terminals, moved panels and user-added views.

The host's successful commit currently means its command process was created,
not that a command such as flotilla attach finished remote authentication.
Readiness beyond that needs a provider signal. Input during held intervals,
history, removal policy, user overrides and workspace-relative open operations
remain separate decisions.
