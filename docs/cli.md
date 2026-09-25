# Command line (`pkd`)

`pkd` is PkgDeck's command-line tool. It uses the same engine as the app.
Running `pkd` with no command prints help. Use `pkgdeck` to open the app.

In the Flatpak build, run it with:

```sh
flatpak run --command=pkd io.github.astrovm.PkgDeck
```

![Package details in the CLI](screenshots/cli.png)

## Quick reference

```sh
pkd sources                              # show package managers and what they support
pkd search neovim --from apt             # search one source
pkd info neovim --from homebrew          # show package details
pkd list --json                          # list installed packages as JSON
pkd install neovim --from apt --yes      # install
pkd remove neovim --from apt --yes       # remove
pkd update --from apt --yes              # refresh package lists (installs nothing)
pkd upgrade neovim --from apt --yes      # update one package
pkd upgrade                              # update everything
pkd clean                                # list cleanup tasks
pkd inspect git                          # find which package provides a command
pkd audit                                # find apps installed more than once
pkd doctor                               # check that PkgDeck can reach your package managers
```

## Global options

| Option | Meaning |
| --- | --- |
| `--from SOURCE` | Only use this source. Repeat it to pick several. |
| `--arch ARCH` | Pick a package architecture, such as `amd64` or `i386` for APT. |
| `--scope user\|system` | Pick the user or system installation. |
| `--yes`, `-y` | Approve changes without asking. |
| `--auth sudo\|polkit` | How to get permission for system changes. Default: `sudo`. |
| `--json` | Print machine-readable JSON. See [JSON output](#json-output). |

Sources for `--from`: `fwupd`, `apt`, `dnf`, `pacman`, `zypper`, `snap`,
`homebrew`, `homebrew-cask`, `appimage`, `flatpak`, `docker`, `podman`,
`cargo`, `npm`, `pnpm`, `bun`, `pip`, `pipx`, `uv`, `composer`, `gem`,
`codex`, `claude`, `grok`, and `opencode`.

Without `--from`, PkgDeck uses every package manager it finds. Package managers
that aren't installed are skipped. Sources that fail are reported, not
hidden. `pkd sources` lists every source, including unavailable ones and why.

## Searching

Search is case-insensitive and matches part of the name or description.
Results are ranked: exact name, then names that start with the query, then
names that contain it, then description matches. Homebrew searches formula and
cask names only.

## Choosing packages

`install`, `info`, `remove`, and `upgrade NAME` need the exact package name. No
fuzzy matching or aliases.

- If the same name exists in more than one source, pick one with `--from`.
- Use `--arch` to choose between APT architectures.
- If a Flatpak is installed for both User and System, add `--scope`:
  `pkd --from flatpak --scope system install org.example.App`
- Homebrew tap packages keep their full name, such as `owner/tap/formula`. PkgDeck
  uses the `brew` found in your `PATH`.
- If a source fails to answer, PkgDeck won't guess which package you meant.
  Retry, or pick a working source with `--from`.

You can pass several names at once. PkgDeck finds every package before
changing anything, then runs each change in order. If one fails, earlier
changes are kept.

`pkd upgrade` without names updates every installed package that has an
update. `--from` and `--arch` narrow it down. `pkd update` only refreshes
package lists. It never installs updates.

## Confirmation and passwords

In a terminal, PkgDeck shows what it will change and asks before doing it.
In scripts, or with `--json`, you must pass `--yes`. Otherwise the command stops
with exit code 2 before doing anything.

Approving a change doesn't give PkgDeck admin rights, and PkgDeck never reads
your password:

- `--auth sudo` (default) only works if `sudo` already has a cached login or a
  password-free rule.
- `--auth polkit` uses your desktop's password prompt.

Homebrew always runs as your user.

### APT

- Installs and single-package upgrades never remove other packages.
- A full upgrade (`pkd upgrade` with APT) does a dry run first and shows every
  update, install, and removal. It checks the plan again right before running.
- If the upgrade would remove packages, review the list and add
  `--allow-removals`. `--yes` alone never approves removals.
- `pkd remove` behaves like normal APT removal.

### Homebrew

Homebrew keeps its own dependency checks, locks, and tap rules. `pkd remove`
runs `brew uninstall --force --formula`, which removes every installed version
of that formula, following
[Homebrew's documented behavior](https://docs.brew.sh/FAQ#how-do-i-uninstall-a-formula).
It never skips dependency checks. Homebrew's automatic update and cleanup are
turned off during package changes. `pkd update` still refreshes Homebrew.

### Docker and Podman

Docker and Podman list your local images. Each image is identified by its
image ID. Docker images are system-wide, while Podman images belong to your
user.

```sh
pkd --from docker upgrade IMAGE_ID                         # pull the image's tag again
pkd --from docker install registry.example/team/image:tag  # pull a new image
pkd --from docker remove IMAGE_ID                          # remove an image
```

Replace `docker` with `podman` for Podman. Registry images only show up when
exactly one container source is selected, so normal searches don't return
container results. PkgDeck never force-removes images used by containers and
never deletes volumes. A tag isn't marked as updated unless the registry is
actually checked.

### Cancelling

Ctrl+C cancels reads and pending changes. If a package manager is already
making changes, it finishes first. The result then shows
`cancellation_deferred`. After a batch with failures, check each result.

## Cleaning up

```sh
pkd clean                     # list cleanup tasks (changes nothing)
pkd clean apt:autoremove      # run one task
pkd clean --all               # run every task
```

Each task has a key like `apt:autoremove` and a preview from the package
manager itself. Running a task uses the same confirmation, permission, and
cancellation rules as other changes. PkgDeck checks the task again right
before running it.

| Source | What it cleans |
| --- | --- |
| APT | Unused dependencies and old downloaded packages |
| Homebrew | Unused dependencies and old downloads |
| Docker, Podman | Untagged images |
| Docker Buildx | Unused build cache |
| npm, pip, uv | Download caches |

- Volumes are never cleaned.
- pip needs a virtual environment, like its other commands.
- pnpm, the Podman build cache, and unused Flatpak runtimes aren't offered,
  because their package managers can't show an exact preview.
- Listing APT tasks doesn't need a password. The list only counts cached
  downloads. APT decides which ones are old when the cleanup runs.

## Inspecting your system

`pkd inspect COMMAND` shows which file runs when you type `COMMAND` and which
package installed it. It searches your `PATH` in order, shows other matches
and symlink targets, and asks your package managers who owns each file.
It never runs the command.

- "unknown" means no package manager claims the file.
- "unmatched" means a package manager claims the file, but PkgDeck can't match
  it to an installed package.
- `--json` includes the full package ID.

`pkd audit` shows apps installed more than once. Copies are grouped only when
they share an AppStream ID or homepage, and versions aren't compared across
package managers. It also lists leftover config files that dpkg recorded for
removed packages. PkgDeck doesn't scan your home folder or guess leftovers.
Use `--from` and `--scope` to narrow the report.

## Exporting your software list

```sh
pkd inventory export software.json               # every installed package
pkd --from apt inventory export apt.json bash    # one package
pkd inventory preview software.json              # check the list on this machine
```

The export is a versioned JSON file with package names, sources, and scopes.
It doesn't include passwords, scripts, user IDs, or paths. Export never
overwrites an existing file.

`preview` checks each entry on the current machine. It reports whether it's
installed, installable, unavailable, unsupported, or ambiguous, without
changing anything. Recorded versions are for reference only. Installing from
an inventory isn't supported yet.

## Repositories

```sh
pkd repos                                         # list repositories
pkd repos list --from flatpak --scope system
pkd repos add example https://example.org/example.flatpakrepo --from flatpak --scope user
pkd repos disable example --from flatpak --scope user
pkd repos enable example --from flatpak --scope user
pkd repos priority example 10 --from flatpak --scope user   # 0–9999, higher wins
pkd repos remove example --from flatpak --scope user
pkd repos edit --from apt                         # open Software Sources
pkd repos enable lvfs --from fwupd
```

Changes need exactly one `--from` and a confirmation (or `--yes`). Flatpak uses
User by default. Add `--scope system` for system repositories. Removing a
Flatpak repository doesn't force-remove apps installed from it. Repositories
keep their normal signature checks.

## Firmware

If `fwupdmgr` is installed, `pkd upgrade` includes firmware updates.

```sh
pkd list --from fwupd                    # list devices
pkd upgrade --from fwupd                 # update all firmware
```

To update one device, use the device ID from `pkd list --from fwupd --json`.
Power and restart requirements are shown before you confirm. PkgDeck never
restarts your computer, downgrades firmware, or forces an update.

## Standalone CLI tools

PkgDeck finds Codex, Claude Code, Grok, and OpenCode when they were installed
with their official installers. They appear in `pkd list` and `pkd upgrade`.

```sh
pkd list --from codex --json
pkd info codex --from codex
pkd upgrade codex --from codex
pkd upgrade --from claude --from grok --from opencode
```

PkgDeck only updates these tools. It can't install or remove them. Copies
installed with npm or Homebrew are handled by that package manager. The
program must be owned by you and installed in the official location:

| Tool | Location | Overrides |
| --- | --- | --- |
| Codex | `~/.local/bin/codex`, linking into `~/.codex/packages/standalone/releases` | `CODEX_HOME`, `CODEX_INSTALL_DIR` |
| Claude Code | `~/.local/bin/claude`, linking into `~/.local/share/claude/versions` | `XDG_DATA_HOME`; `CLAUDE_CONFIG_DIR` for the update channel |
| Grok | `~/.grok/bin/grok`, linking into `~/.grok/downloads` | `GROK_BIN_DIR` |
| OpenCode | `~/.opencode/bin/opencode` | |

How updates are checked and installed:

| Tool | Checks with | Updates with |
| --- | --- | --- |
| Codex | Latest stable release | Official installer, run from a private temporary folder |
| Claude Code | Your configured stable or latest channel | `claude update` |
| Grok | `grok update --check --json` | Grok's own updater |
| OpenCode | Latest stable release | `opencode upgrade VERSION --method curl` |

`curl` is needed for the release checks and the Codex installer. If a check
fails, it's reported as an error, never as "up to date". Updates need
confirmation, run without admin rights, and never downgrade a newer version.
PkgDeck checks the version after updating. Restart open sessions of the tool
to use the new version.

Official docs: [Codex](https://learn.chatgpt.com/docs/codex/cli),
[Claude Code](https://code.claude.com/docs/en/setup),
[Grok](https://docs.x.ai/build/enterprise),
[OpenCode](https://opencode.ai/docs/cli/#upgrade).

## JSON output

With `--json`, `pkd` prints exactly one JSON document to stdout. Progress goes
to stderr.

```json
{"schema_version":1,"exit_code":0,"data":{"packages":[],"failures":[]}}
```

| Command | `data` contains |
| --- | --- |
| `search`, `list` | `packages`, plus `failures` for each source that failed |
| `info` | Package details |
| `sources` | `sources` |
| `clean` | Cleanup `items` and `failures` |
| Changes | `operations`, each with a `result` of `{"Ok":...}` or `{"Err":...}` |
| Errors | `error`, and a readable `message` when available |

- Package IDs have `backend`, `name`, `architecture`, and `scope`, plus remote
  and ref fields when the source needs them.
- Versions are the package manager's own strings. `update` is `unknown`,
  `current`, or `available`.
- If an APT full upgrade would remove packages and `--allow-removals` wasn't
  passed, the error is `apt_removals_require_consent` and includes the `plan`.
- A failed source includes the package manager's own error message when
  available. Run the same command directly in a terminal for full output.
- `doctor` is text-only. `doctor --json` exits with 2. Help, version, and usage
  errors are always plain text.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success, including no results or no updates |
| 1 | A package manager or other error |
| 2 | Invalid usage, or `--yes` is required |
| 3 | No package with that exact name |
| 4 | More than one match, or a source failed to answer |
| 5 | Permission denied or unavailable |
| 6 | APT is busy (locked by another program) |
| 7 | Cancelled or declined |
| 8 | Some sources failed, or some changes succeeded and others failed |

If every change in a batch fails, the exit code is the first failure's code.
`pkd sources` lists unavailable package managers as normal output. It only
returns 1 if detection itself fails.
