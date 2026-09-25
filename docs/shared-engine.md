# Package engine

`pkgdeck-core` is the engine behind both the app and `pkd`. It handles package
identity, finding package managers, choosing packages, and running changes. It
doesn't depend on Qt or on CLI parsing. The engine is synchronous and owns its
`Send` backends. The GUI runs queries on worker threads and receives typed
results and progress events. For CLI behavior, see the [CLI guide](cli.md).

## Package identity

A `PackageId` is made of:

- the backend ID
- the backend's own package name
- the architecture
- the scope: system, a user (by UID), or a chosen environment path
- optional remote and native ref fields

Backends must return stable names and canonical environment paths. Display
names are only for showing to people. They're never part of the identity.

A `Package` has a summary, optional installed and available versions, and an
update state: `unknown`, `current`, or `available`. Versions are plain strings.
Each backend compares them its own way. `PackageDetails` adds a description,
homepage, and dependencies.

A `Selector` matches the exact package name, optionally narrowed by backend,
architecture, and scope. It never assumes two packages are the same app just
because their names match.

- No match returns `NotFound`.
- More than one identity returns `Ambiguous` with every candidate.
- The same identity reported twice counts once.

## Backends

| Method | What it does |
| --- | --- |
| `id` / `capabilities` | Stable backend ID and the operations it really supports |
| `detect` | Whether the package manager is available, or a typed error |
| `search` | Packages matching a search term |
| `installed` | Installed packages with their versions |
| `details` | Details for exactly the requested package |
| `cleanup` | Safe cleanup tasks, found with the manager's own dry-run or list commands |
| `apt_upgrade_plan` | Dry run of an APT full upgrade, for review before running |
| `execute` | Runs an install, remove, refresh, upgrade, or cleanup, with progress |

Every method except `detect` defaults to "unsupported". Before calling a
backend, the engine checks for cancellation, registration, capabilities, and
availability. Registering the same backend twice is rejected. The first one
stays.

Backends should use documented APIs and structured output. Commands go through
the [host execution layer](host-execution.md) with full paths and argument
lists, and errors are passed up as `EngineError`. The engine never runs a
shell and has no frontend-specific code.

The engine rejects a whole backend result if it has packages from another
backend, duplicate identities, or installed packages with no installed
version. It also rejects details for a different package, including a
different scope.

## Partial results

`search` and `installed` return a `PackageReport` with the packages found, the
sources that worked, and an error for each source that failed. One failed
source doesn't hide results from the others. Sources and packages are always
returned in the same order.

If a relevant source failed, selecting a package by name returns `Incomplete`,
even when another source has a match. This stops PkgDeck from installing the
only visible result of an incomplete search. If you explicitly pick a working
backend, failures in other backends are ignored. Failures in the chosen backend
still block.

## Streaming and cached detection

`search_stream` queries every backend in parallel and sends the sorted results
so far each time a backend answers. The last update is identical to the
synchronous `search`. The GUI shows partial results so it feels fast, while
the final result stays predictable. Backends go back to the engine afterward
for reuse. Cancellation works per backend, the same as a normal query, and
nothing is rolled back. The synchronous `search` and `installed` used by the
CLI are unchanged.

`native_engine` remembers successful detection results, so the next query
doesn't detect again. Failed detections aren't remembered and are retried.
Engines only live for one query, so the cache can't go stale. `details_reuse`
does the same for details on an engine that has already run a query. Use
`details` on a fresh engine, which checks availability first.

## Running changes

`Operation::Refresh` refreshes one backend's package lists.
`Operation::Upgrade` updates one exact installed package. The engine never
swaps one for the other.

**APT `UpgradeAll`** needs a reviewed plan. The engine runs a fresh dry run and
compares it before running `dist-upgrade`. If the plan is missing, changed, or
incomplete, it stops. The GUI builds the preview on a worker thread so the
window stays responsive.

**Single changes.** Backends can also return an `operation_plan` for one
change. By default there is none. APT dry-runs single installs, removals, and
updates and reports the package changes. Size and restart info stay unknown
when APT doesn't provide them. The engine checks the plan again before
running. If it changed, the GUI asks for confirmation again.

**Events.** Each change sends `Started`, then any progress events, then exactly
one `Finished` with the result. `Started` covers pre-checks. It doesn't mean
authorization or changes have begun. Progress has messages and transfer counts
with optional totals. Backends shouldn't make up percentages when there's no
total.

**Batches.** `execute_batch` checks every change and its saved plan first. If
any check fails, every change gets an error and nothing runs. When the bundled
helper is available, it asks for permission once, before any change, and only
allows the commands in that batch. Consecutive APT changes of the same kind
run in one APT transaction but still get one result each.

- Results come back in request order, one per request.
- A failure doesn't stop the rest, and finished changes aren't undone.
- Changes after a cancellation get a cancelled result and a final event.
- If cancellation arrives while a package manager is already making changes,
  that change finishes and is reported as successful. It isn't reported as
  rolled back.

Backends must pass the cancellation token to their commands and keep this
distinction.

**Cleanup.** A cleanup task is identified by a backend and a fixed key. It
never contains shell code or arbitrary paths. A backend only supports `Clean`
if it can find a plan without changing anything, and map the key back to a
fixed command. The app and CLI show the preview and ask before running it.
Backends that can't do this return `Unsupported`.

## Tests

```sh
cargo test --locked -p pkgdeck-core --test engine
```

Fake in-memory backends test:

- detection and choosing between sources
- telling apart architectures, users, and environments
- details and installed state
- install → refresh → upgrade → remove
- unsupported actions
- availability, query, and change failures
- malformed responses
- cancellation and event order

The tests check that refresh doesn't upgrade anything, and that finished
changes are kept when a later one fails. They never run a real package
manager or change your system.
