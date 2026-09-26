# PkgDeck support expansion report

Research date: 2026-09-26. Reviewed `main` at `67b85b4` after
`git pull --ff-only origin main` (already current). This is a product and
implementation assessment, not a test of these integrations on a Mac.
Priorities and effort ratings below are engineering judgments, not usage
statistics. External capabilities were checked against the linked primary sources.

**Recommendation:** prioritize unmanaged-app discovery and an explicit
“Manage with Homebrew” workflow, improve AI-tool discovery through existing
managers, and add mise as the first substantial new developer-tool backend.
Investigate Linux Homebrew casks immediately: upstream now supports them,
but PkgDeck registers its cask backend only on macOS.

## Current coverage and actual gaps

The source registry contains **25 adapters**:

| Area | Existing sources |
| --- | --- |
| System packages | APT, DNF, Pacman, Zypper |
| App distribution | Flatpak, Snap, AppImage, Homebrew formulae, Homebrew casks |
| Developer packages | Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems |
| Other | Docker images, Podman images, fwupd |
| Standalone AI CLIs | Codex, Claude Code, Grok, OpenCode |

These adapters have different capabilities. Standalone AI sources discover
recognized installations and update them; they do not install or remove them.
Package-manager installations of those tools belong to their existing manager.
Pip coverage is scoped to virtual environments. AppImage already supports
managed imports and discovery of external copies through desktop entries.

The reusable foundations are good: exact package identities, environment scopes,
capability checks, confirmation plans, host execution, partial results, command
inspection, and duplicate-copy auditing. However:

- There is no general macOS `.app` inventory or adoption operation.
- Homebrew casks are gated by `cfg!(target_os = "macos")`.
- Cask metadata currently retains a small subset: token, name, homepage,
  version, installed version, and outdated state. Adoption needs artifact and
  installation-location information too.
- `pkd inspect` resolves executable paths, but its native ownership queries
  currently cover dpkg, RPM, and Pacman. Extend authoritative ownership for
  Homebrew and other new sources before relying on it for migration.
- Duplicate grouping is display-only. It must not authorize replacement.
- `Operation` has no ownership-transfer operation; ordinary batches have no
  rollback guarantee. A migration cannot safely be represented as an ordinary
  remove/install batch.

Code evidence: [source registry and adapters](../crates/pkgdeck-core/src/backends/mod.rs),
[standalone adapter](../crates/pkgdeck-core/src/backends/standalone.rs),
[inspection](../crates/pkgdeck-core/src/inspection.rs),
[operations and identity](../crates/pkgdeck-core/src/package.rs),
[engine contract](shared-engine.md), and [CLI coverage](cli.md).

## Prioritized opportunities

Effort: S = bounded catalog/adapter change; M = adapter plus UI and lifecycle
tests; L = new ownership, transaction, or platform behavior. These are relative
sizes, not delivery estimates. P0 is foundational; P1 follows; P2 needs demand
or a dedicated design.

| Priority | Opportunity | User benefit | Effort | Recommended first scope |
| --- | --- | --- | --- | --- |
| P0 | Unmanaged macOS app inventory and Brew matching | Makes software outside package databases visible | M | Read-only inventory and reviewed matches |
| P0 | AI application catalog using current managers | Easier discovery without duplicate backends | S–M | Gemini, Copilot, Aider; source and channel labels |
| P0 | Linux Homebrew cask compatibility | Expands an existing integration | M | Capability probe and compatible-artifact tests |
| P1 | Manage existing Mac apps with Homebrew | Converts manual installs into tracked installations | L | Identical-artifact adoption for an allowlist |
| P1 | Cursor CLI standalone updates | Covers an official installer outside current sources | M | Verified inventory; updates after updater validation |
| P1 | mise | Runtime and developer-tool coverage across Linux/macOS | M–L | Inventory, then explicit global-tool operations |
| P1 | Mac App Store via mas | Covers apps that should retain App Store ownership | M | Inventory and selected updates |
| P1 | Conda, followed by mamba/micromamba | Scientific and data-development environments | L | Explicit environment inventory and solver previews |
| P2 | MacPorts | Completes another macOS package ecosystem | M | Native package lifecycle with variants preserved |
| P2 | Rustup | Toolchain coverage beyond Cargo-installed executables | M | Installed toolchains and channel-aware updates |
| P2 | Nix profiles | Useful cross-platform package coverage | L | User profiles only; no declarative configuration edits |
| P2 | apk and XBPS | Extends distro coverage | M each | CLI-first integration and distro fixtures |
| P2 | rpm-ostree | Supports immutable desktops | L | Deployment inventory and pending-reboot state first |
| P2 | AUR via an existing helper | Broader Arch application availability | L | Search/inventory before build execution |

### Package-manager feasibility

**mise is the strongest first addition.** Its inventory has JSON output and
records multiple tool versions and their configuration sources. Its upgrade
command exposes tool upgrades. Start with explicitly selected global tools;
project configuration, pins, and installed-but-inactive versions must remain
distinct. A tool installed through mise's npm backend must not also be claimed
as a global npm installation. Preserve the controlling manager and configuration
path. [Inventory](https://mise.jdx.dev/cli/ls.html),
[upgrades](https://mise.jdx.dev/cli/upgrade.html).

**Conda is valuable but requires environment transactions.** Its update command
supports JSON and dry-run output. An update can change several packages, so show
the solver's full plan and revalidate it before execution. Use canonical prefix
paths as scope; retain channels and build identities, and never silently update
every environment. Start with Conda; test mamba and micromamba separately before
advertising compatibility. [Conda update](https://docs.conda.io/projects/conda/en/stable/commands/update.html).

**mas fills a different need from casks.** It inventories and updates App Store
apps, with Apple Account and privilege requirements for mutations. Its accurate
outdated check initiates a download to read metadata; do not describe that as a
purely local read. Start with inventory and clearly labeled update checking;
let Apple handle authentication. [mas documentation](https://github.com/mas-cli/mas).

**MacPorts** has native installed/outdated/upgrade operations. Keep variants,
inactive versions, and dependency effects in the model; do not consolidate its
packages into Brew by name. [MacPorts guide](https://guide.macports.org/#using.port).

**Rustup** manages Rust toolchains separately from Cargo packages. Preserve
stable/beta/nightly and pinned toolchains. Separate toolchain updates from
rustup's own update, which can be suppressed per invocation.
[Rustup basics](https://rust-lang.github.io/rustup/basics.html).

**Nix** offers JSON profile inventory including flake references and store paths.
Limit the first backend to explicit user profiles. Do not mutate NixOS or
Home Manager configuration, infer upgrades from display versions, or make
global garbage collection a routine cleanup task.
[Nix profile list](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-profile-list.html).

**apk and XBPS** are reasonable distro extensions, but available package commands
do not prove PkgDeck's current GUI/build dependencies work on those systems.
Validate the engine and host authorization first, then distribution support.
[Alpine APK](https://wiki.alpinelinux.org/wiki/Apk),
[Void XBPS](https://docs.voidlinux.org/xbps/index.html).

**rpm-ostree** provides JSON status and deployment-based changes. Model booted
and pending deployments and reboot requirements rather than presenting it as
ordinary live RPM updates. [Upstream handbook](https://github.com/coreos/rpm-ostree/blob/main/docs/administrator-handbook.md).

**AUR** requires build-recipe review and helper-specific behavior. Integrate one
installed helper first, retain its review/diff step, and distinguish AUR origin
from Pacman's installed-package ownership. Avoid duplicate rows for the same
installed package. [ArchWiki AUR helpers](https://wiki.archlinux.org/title/AUR_helpers).

Windows managers such as WinGet, Scoop, and Chocolatey should be a separate
platform initiative. Current Unix host execution and Linux/macOS distribution
support make them more than additional command adapters. Similarly, defer
additional runtime managers until mise establishes multi-version semantics.

## AI CLIs: expand coverage without multiplying sources

| Tool | Coverage today | Proposed addition |
| --- | --- | --- |
| Codex, Claude Code, Grok, OpenCode | Explicit standalone updater adapters; manager-owned copies use their manager | Improve ownership explanations, duplicate detection, channel display, and fixture maintenance |
| Gemini CLI | Official npm/Homebrew install routes fit existing adapters | Catalog identity for `@google/gemini-cli` and `gemini-cli`; lifecycle verification |
| GitHub Copilot CLI | Official npm/Homebrew routes fit existing adapters; official standalone route is not covered | Catalog identity for `@github/copilot`; standalone discovery later |
| Aider | Official uv/pipx-style routes fit existing tool-management scope | Catalog entry for `aider-chat`, preserve isolated environment and dependency options |
| Cursor CLI | No dedicated standalone adapter | Detect the official installation, verify identity, then support its updater |
| Ollama | Manager-installed packages can use existing adapters | Separate desktop app, CLI/service, and model inventory; defer native Linux service migration |

“Fits existing adapters” is an architectural assessment, not a claim that these
specific applications have passed PkgDeck end-to-end tests. Add targeted fixtures
and real lifecycle smoke tests before advertising support.

Official evidence: [Gemini installation](https://geminicli.com/docs/get-started/installation/),
[Copilot installation](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli),
[Aider installation](https://aider.chat/docs/install.html).

Cursor's current documentation uses `agent --version` and `agent update`; older
documentation uses `cursor-agent`. A generic executable named `agent` is not
sufficient ownership evidence. Require a recognized installation layout and
upstream identity, and verify whether a side-effect-free update check exists
before adding automatic update discovery.
[Current Cursor CLI installation](https://cursor.com/docs/cli/installation).

Ollama's native Linux installation involves libraries and potentially a system
service; its documented update reruns installation. Treat this as a separate
service-aware integration. On macOS, distinguish the `ollama` formula from
the `ollama-app` cask. Model downloads should have their own size and removal
semantics, not be confused with updates to the program.
[Linux installation](https://docs.ollama.com/linux),
[formula](https://formulae.brew.sh/formula/ollama),
[app cask](https://formulae.brew.sh/cask/ollama-app).

The proposed catalog maps a product to exact package IDs, channels, platforms,
and evidence. It improves search and explains available install routes; the
selected manager remains responsible for installation. Do not turn a marketing
name or shared homepage into permission to replace another installation.

## Common standalone applications

Start with an inventory layer independent of update execution. A useful row
contains installation path, observed version, owner, provenance confidence,
available management options, and why an operation is unavailable.

On macOS, scan `/Applications`, `~/Applications`, and user-selected directories.
Read bundle metadata without launching apps. Track bundle identifier, short and
build versions, architecture, signature identity, App Store receipt presence,
and matching package receipts. Exclude nested helper apps from the main list.
Treat managed-device or installer-owned applications as requiring a different
workflow. On Linux, extend existing desktop-entry/AppImage discovery with
selected application directories; do not claim arbitrary files in `/opt`.

The following have verified Homebrew casks and make a useful evaluation set.
Their presence in Homebrew does **not** establish safe adoption of every
existing installation.

| App | Cask | Initial handling |
| --- | --- | --- |
| Visual Studio Code | [visual-studio-code](https://formulae.brew.sh/cask/visual-studio-code) | Early candidate; check `code` launcher conflicts |
| Firefox | [firefox](https://formulae.brew.sh/cask/firefox) | Early candidate; distinguish stable, ESR, beta, and language |
| Obsidian | [obsidian](https://formulae.brew.sh/cask/obsidian) | Early candidate; preserve vaults and settings |
| Discord | [discord](https://formulae.brew.sh/cask/discord) | Candidate after self-updater behavior is tested |
| Spotify | [spotify](https://formulae.brew.sh/cask/spotify) | Candidate after self-updater behavior is tested |
| Slack | [slack](https://formulae.brew.sh/cask/slack) | Distinguish direct-download and App Store editions |
| Google Chrome | [google-chrome](https://formulae.brew.sh/cask/google-chrome) | Later; validate updater and managed-install interactions |

Avoid building a universal updater for arbitrary `.app`, DMG, PKG, ZIP, or
tarball installations. A supported package manager or a verified upstream
updater should own mutations. Sparkle is an app update framework, not proof
that every bundled application offers a uniform external update command.
[Sparkle documentation](https://sparkle-project.org/documentation/).

## Moving standalone Mac apps to Homebrew

**Yes, support this, as an explicit ownership-transfer feature.** Cask
availability alone is insufficient. Homebrew documents `--adopt` for existing
artifacts identical to the artifacts being installed, and it cannot be combined
with `--force`. It is not a general registration command for an arbitrary
existing version. [Homebrew manual](https://docs.brew.sh/Manpage).

Proposed flow:

1. **Discover and match.** Use a curated mapping plus bundle identifier,
   publisher signature, edition/channel, architecture, and destination.
   Display the matching evidence. Name similarity only suggests candidates.
2. **Preview.** Show installed and target versions, current and future manager,
   app path, launcher changes, download, need to close the app, and recovery
   location. Separate app data from the app bundle.
3. **Revalidate.** Check the selected Brew executable/prefix, target metadata,
   bundle fingerprint, current owner, and running-app state again. An upstream
   self-update between preview and execution invalidates the plan.
4. **Adopt identical artifacts.** Run a narrowly authorized
   `brew install --cask --adopt TOKEN` operation and verify the resulting Brew
   record and artifact path. If Homebrew rejects the match, stop and explain.
5. **Handle a different version separately.** Offer a reviewed replacement
   workflow only for supported simple app bundles: stage the old bundle in a
   recovery location, install through Brew, verify, and retain recovery until
   accepted. Do not fall back automatically from failed adoption to replacement.
6. **Verify ownership.** Refresh both inventories, check launcher resolution,
   remove duplicate presentation of the same physical installation, and record
   the migration outcome and any recovery steps.

This replacement workflow is a proposal requiring macOS prototyping. It is not
an existing Homebrew transaction or a guarantee that every installer can be
rolled back. Installer packages, helper services, privileged components,
MDM-managed apps, App Store editions, and modified bundles are outside the MVP.
Do not use `--force`, `--zap`, or recursive deletion to resolve adoption conflicts.
Do not remove preferences, accounts, credentials, browser profiles, or documents.

For standalone CLI-to-Brew migration, use a distinct plan: establish the new
installation, verify its command/version, resolve PATH shadowing, and only then
offer removal of the exact old installation. Existing standalone adapters have
no uninstall contract, so the first version should explain manual cleanup rather
than invent one. Never assume `.local/bin` symlinks imply ownership of an entire
configuration directory.

**Update ownership after migration:** Homebrew's current FAQ describes version
comparison for eligible self-updating app bundles and cases it cannot reliably
compare. Keep app-observed and manager-recorded versions separate; test supported
Brew versions rather than forcing greedy updates globally. Migration does not
necessarily disable the vendor's updater.
[Homebrew FAQ](https://docs.brew.sh/FAQ).

## Linux casks: an immediate compatibility investigation

Current Homebrew documentation explicitly permits Linux casks, with Linux
`app_image` artifacts and portable binaries. Obsidian's cask currently includes
Linux AppImages. PkgDeck's macOS-only registration therefore excludes a real
upstream capability. [Cask cookbook](https://docs.brew.sh/Cask-Cookbook),
[acceptable casks](https://docs.brew.sh/Acceptable-Casks),
[Obsidian cask](https://formulae.brew.sh/cask/obsidian).

Do not merely remove the platform check. Probe the installed Brew's support,
filter incompatible artifacts, retain its installation scope, and test search,
install, update, and removal on both Linux architectures. In particular, ensure
PkgDeck's external-AppImage discovery does not offer to import or remove a
Homebrew-owned AppImage as though it were unmanaged. The minimum supported Brew
version and complete artifact compatibility still need validation.

## Suggested implementation sequence and acceptance criteria

1. **Foundation:** add authoritative Homebrew ownership and read-only macOS app
   inventory. Ship candidate matching with evidence and no automatic mutations.
   Introduce the AI catalog using existing package routes. Prototype Linux casks.
2. **Bounded additions:** mise inventory and global-tool management, Cursor CLI
   discovery/update validation, and mas inventory/selected updates.
3. **Mac adoption MVP:** three curated app families (VS Code, Firefox stable,
   Obsidian), identical-artifact adoption, exact preconditions, and postchecks.
   Release replacement/recovery only after interruption testing.
4. **Broader ecosystems:** Conda environment plans, then MacPorts/Rustup; pursue
   Nix, immutable systems, AUR, apk, and XBPS according to user demand.

Implementation should extend the core engine and host command authorization,
then expose the same behavior through CLI and GUI. Update the source registry,
native engine registration, CLI parser, GUI source catalog, documentation, and
fixtures together. Prefer a typed migration plan/service over adding implicit
cross-manager behavior to `Install` or `Remove`.

Required behavioral checks:

- Manager-owned and standalone copies coexist without double ownership.
- Same display name, wrong publisher, wrong channel, and wrong architecture
  cannot authorize migration.
- A changed bundle or changed target after preview blocks execution.
- Missing metadata remains unknown; it never becomes “up to date.”
- Self-updated apps newer than the cask are not silently downgraded.
- Adoption rejection leaves the original app intact.
- Replacement interruption has a recorded, tested recovery path; app data stays
  unchanged in success and failure cases.
- Intel and Apple Silicon Brew prefixes, custom app directories, launcher
  conflicts, running apps, and multiple installed versions are covered.
- New managers preserve environment pins, configuration sources, and full
  dependency plans; “Update all” does not escape the selected scope.
- Absent sources remain cheap to detect. Benchmark startup and background
  checks so new adapters do not undo the recent performance work.

No application code or installed software was changed for this assessment.
The next concrete deliverable should be the inventory/matching foundation and
a Linux-cask compatibility spike, followed by the narrowly scoped adoption MVP.
