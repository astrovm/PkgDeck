# Shared package engine

Step 3 adds the package model and backend contract to `pkgdeck-core`, with no Qt,
CLI parsing, or new dependencies. The engine is synchronous and owns `Send`
backends; frontends should run it on a worker and consume its typed results/events.
Step 4 adds real APT/Homebrew adapters and public package commands; see the
[CLI contract](cli.md).

## Identity and metadata

`PackageId` consists of backend ID, backend package identifier, architecture, and
installation scope. Scopes distinguish the system, a user UID, and an explicitly
selected environment path. Adapters must supply stable identifiers and canonical
host environment paths. Display names are presentation data, not identity.

`Package` carries summary information, optional installed/candidate versions, and
explicit update availability: unknown, current, or available. Version strings are
opaque: each backend applies its own version comparison rules. `PackageDetails`
adds description, homepage, and dependency identifiers.

A `Selector` matches the exact backend package identifier, with optional backend,
architecture, and scope constraints. It never infers application equivalence from
a matching name. Zero matches produce `NotFound`; multiple identities produce
`Ambiguous` with all candidates. Repeated copies of the same identity do not make
an additional candidate.

## Backend contract

| Method | Responsibility |
| --- | --- |
| `id` / `capabilities` | Stable backend registration ID and the operations actually supported |
| `detect` | Available/unavailable status, or a typed failure |
| `search` | Structured package results for a search term |
| `installed` | Structured package results with installed versions |
| `details` | Details for exactly the requested identity |
| `execute` | A typed install, remove, metadata refresh, or package upgrade, with progress |

Methods other than detection have explicit unsupported defaults. Before dispatch,
the engine checks cancellation, registration, capabilities, and availability.
Duplicate backend registrations are rejected without replacing the existing one.

Adapters should prefer documented APIs and structured output. Command adapters
use the [host execution boundary](host-execution.md) with absolute executables
and argument arrays, and propagate its typed errors through `EngineError`.
There is no frontend-specific package-manager logic or shell execution in the engine.

The engine rejects a query's entire backend result if it contains foreign or
duplicate identities, or an installed listing without an installed version. It
also rejects details for a different identity, including a changed scope.

## Partial results and selection

Search and installed listing return a `PackageReport` with successful packages
and per-backend failures. One failed source does not discard another source's
results. Backend traversal and package ordering are deterministic.

A name cannot safely resolve while a relevant source's query failed. Selection
therefore reports `Incomplete`, even if one successful source has a matching
package. Explicitly selecting a successful backend ignores failures in other
backends; failures in the chosen backend still block selection. This prevents
silently installing the only visible result from an incomplete search.

## Operations and progress

`Operation::Refresh` targets one backend's metadata. `Operation::Upgrade` targets
an explicit installed package identity. These are different capabilities and
requests; the engine never substitutes one for the other.

Each dispatch emits `Started`, zero or more backend progress events, and exactly
one `Finished` event carrying its result during normal error-returning execution.
`Started` includes preflight checks and does not imply authorization or a native
write has begun. Progress includes messages and transfer counts with optional
totals; adapters should not invent percentages when a manager supplies no total.

`execute_batch` preserves request order and returns one result per request. It
continues after failures and does not roll back successful native operations.
Pending requests after cancellation receive cancellation results and terminal
events. Cancellation during a native write is controlled by the host boundary:
a completed write with deferred cancellation stays a successful completion,
not a claimed rollback. Backends must forward the cancellation token to their
underlying operations and preserve this distinction.

## Local verification

```sh
cargo test --locked -p pkgdeck-core --test engine
```

The in-memory synthetic backends exercise discovery, cross-source selection,
architecture/user/environment disambiguation, details, installed state, the
install → refresh → upgrade → remove lifecycle, unsupported capabilities,
availability/query/write failures, malformed responses, cancellation, and event
ordering. Their state is inspected after operations to verify refresh does not
upgrade packages and partial batch failure preserves completed writes. These tests
never invoke a real package manager or alter host package state.
