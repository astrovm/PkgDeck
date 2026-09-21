# APT and Homebrew CLI

Step 4 exposes the shared engine through `pkd`. Native and AppImage execution
use the host boundary. Flatpak and Snap host operations remain explicitly disabled.
`pkd` without a subcommand prints help and exits successfully, including with
redirected input/output. Use `pkgdeck` to launch the GUI.

## Commands and selection

```sh
pkd sources
pkd search neovim --from apt
pkd info neovim --from homebrew
pkd list --json
pkd install neovim --from apt --yes
pkd update --from apt --yes
pkd upgrade neovim --from apt --yes
pkd remove neovim --from apt --yes
```

`--from` accepts `apt`, `dnf`, `pacman`, `zypper`, `snap`, `homebrew`, `appimage`, `flatpak`, `cargo`, `npm`, `pnpm`, `bun`, `pip`, `pipx`, `uv`, `composer`, or `gem`, and repeats to select several sources. Without it, queries cover detected managers;
missing optional managers are omitted, while detection/query failures remain
visible. `sources` includes unavailable managers and their reasons. Search uses
literal case-insensitive substrings: APT names/summaries and Homebrew formula names.
Results rank best-match-first (exact name, name prefix, name substring, then
summary matches); unverified name guesses sort last.
Homebrew identities preserve tap-qualified names (for example
`owner/tap/formula`). Aliases and fuzzy matches are not installation selectors.

Install, info, remove, and named upgrades require an exact package identifier.
Matching identifiers across managers are ambiguous until a single `--from` pins the source.
Use `--arch` to distinguish APT architectures. Homebrew's detected prefix is part
of each identity; the CLI operates on the `brew` executable in the invoking user's
host PATH. It does not merge packages across prefixes or treat equal names as the
same application. Failed source queries prevent potentially ambiguous selection.

`install`, `remove`, and `upgrade` accept multiple names. Resolve all names before
any write; then execute each distinct operation in order. Execution failures do
not roll back successful operations. `upgrade` without names selects all installed
packages with native-reported updates, optionally restricted by `--from`/`--arch`.
`update` refreshes metadata and never upgrades installed packages itself.

## Confirmation and authorization

Non-interactive and `--json` writes require `--yes`; otherwise they return exit 2
before looking up packages. In a terminal, show the resolved operations and ask
for approval of native package and dependency changes. Approval does not grant
system privileges. The default `--auth sudo` uses an existing `sudo -n` grant;
`--auth polkit` uses an existing desktop agent/policy. The frontend does not read
passwords, change authorization policy, or elevate Homebrew.

APT uses `--no-remove` for installations/upgrades and `--only-upgrade` for targeted
upgrades. Explicit removal retains APT's normal dependency behavior. Homebrew
retains its native dependency checks, locks, and tap trust policy. Removal uses
`brew uninstall --force --formula` to remove every installed version of the selected
formula, including older kegs retained after an upgrade; it does not pass
`--ignore-dependencies`. This follows Homebrew's
[documented removal behavior](https://docs.brew.sh/FAQ#how-do-i-uninstall-a-formula). Automatic
Homebrew updates and post-install cleanup are disabled for package operations;
explicit `update` still refreshes Homebrew and tap metadata.

Ctrl-C cancels reads and pending operations. A native write already in progress
finishes before cancellation takes effect; its result records
`cancellation_deferred`. Inspect individual results after any failed batch.

## Output contract, version 1

`--json` emits exactly one document to stdout for application results:

```json
{"schema_version":1,"exit_code":0,"data":{"packages":[],"failures":[]}}
```

Search/list data contain `packages` and per-backend `failures`; info returns package
details; sources returns `sources`; writes return `operations`, each with its
operation and `result` (`{"Ok":...}` or `{"Err":...}`). Selection and other top-level
failures contain `error` and, when available, a human-readable `message`.
A failing source row means the underlying manager errored: the message carries
the manager's own diagnostic where available, and the same command run directly
in a terminal shows complete output. Failed sources block ambiguous selection
and batch upgrades until every queried source answers.
Package IDs always include `backend`, `name`, `architecture`, and `scope`.
Versions remain native strings and update availability is `unknown`, `current`,
or `available`. Native version semantics determine update availability.

Progress goes to stderr. Human mode prints indented records with the same data.
`doctor` is a text-only diagnostic; `doctor --json` returns exit 2. Clap's usage,
help, and version responses keep their normal text behavior, including malformed
arguments supplied with `--json`.

| Exit | Meaning |
| --- | --- |
| 0 | Success (including empty query results or no available updates) |
| 1 | Backend, availability, metadata, or other execution failure |
| 2 | Invalid usage or required non-interactive confirmation missing |
| 3 | No exact matching package |
| 4 | Ambiguous or incomplete selection |
| 5 | Authorization denied/unavailable |
| 6 | APT lock busy |
| 7 | Cancelled or confirmation declined |
| 8 | Query has source failures, or a write batch has both successes and failures |

A batch where every operation fails returns the first operation's failure code.
Source discovery reports unavailable managers as data; detection errors return 1.

## Native metadata contracts and validation

APT metadata comes from the companion `pkgdeck-apt-query` executable through
libapt-pkg's read-only cache and native candidate policy. Packages include the
helper and its shared-library dependencies. Source builds use `scripts/build-apt.sh`
with the distribution's `libapt-pkg-dev` package. Keep the helper beside the
frontend binaries. All APT writes use the fixed `/usr/bin/apt-get` authorization
boundary; the helper only reads metadata and never requests elevated privileges.

Homebrew formula metadata uses `brew formulae` and `brew info --json=v2 --formula`
from the [Homebrew command interface](https://docs.brew.sh/Manpage). Casks are
excluded. Reads have a 120-second deadline and a 32 MiB per-stream limit; truncated
or malformed metadata is an error, not an empty result. Native write output is
bounded while the child is drained to completion.

Local adapter and CLI tests use synthetic metadata and executables. The disposable
VM additionally installs synthetic APT and Homebrew packages at 1.0, refreshes to
2.0, verifies update detection and upgrade, and removes them. Final state is
checked through `dpkg-query`, `brew info`, and fixture-owned files. Homebrew 6.0.22
runs unprivileged at its standard Linux prefix with a controlled local tap. The
fixture trusts only that tap inside the guest. See [host execution validation](host-execution.md#reproducing-validation).
