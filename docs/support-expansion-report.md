# PkgDeck support expansion report

Research date: 2026-09-26; ecosystem facts re-checked 2026-09-27 (see
[What changed since the first draft](#what-changed-since-the-first-draft)).
Reviewed `main` at `67b85b4` after
`git pull --ff-only origin main` (already current). This is a product and
implementation assessment, not a test of these integrations on a Mac.
Priorities and effort ratings below are engineering judgments, not usage
statistics. External capabilities were checked against the linked primary sources.

Review update: checked against implementation commit `5dc077f` in
[PR #114](https://github.com/astrovm/PkgDeck/pull/114) and Homebrew **7.0.6**.
The coverage table below describes the original `67b85b4` baseline. PR #114 adds
a 26th source, `macos-apps`: read-only bundle discovery, exact-path ownership
evidence from the active Homebrew prefix, and bundle-ID cask candidates. It does
not verify publisher signatures, channels, architecture, or artifact equality;
support for user-selected directories and adoption is still proposed. Native
macOS execution is validated through native GitHub-hosted runners and, since
2026-09-27, on a development Mac (below). The initial native inventory/cask lifecycle passed
on both Intel and Apple Silicon in [CI run 36280176056](https://github.com/astrovm/PkgDeck/actions/runs/36280176056).
The expanded suite and its remaining gaps are tracked in the
[platform test audit](platform-test-audit.md); consult the exact PR commit's
checks for its current result.

**Recommendation:** make Linux Homebrew casks and mise the first new work,
add the current AI CLIs through existing managers plus a few standalone
adapters (Antigravity CLI, Copilot CLI, Cursor CLI, Kiro CLI), and keep the
“Manage with Homebrew” adoption workflow behind verified identity checks and
tested recovery.

## What changed since the first draft

Checked against vendor docs, release notes and the Homebrew API on 2026-09-27.

- **Gemini CLI is deprecated.** Google ended it for consumer accounts on
  2026-06-18; Antigravity CLI (`agy`, cask `antigravity-cli`) replaces it. The
  `antigravity` cask is the desktop app, not the CLI. The `gemini-cli` formula is
  deprecated and will be disabled on 2026-12-18.
  [Google](https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli/),
  [Antigravity CLI](https://antigravity.google/docs/cli/install/)
- **Other AI CLI changes:** Amazon Q Developer CLI became Kiro CLI; Cursor's
  command is now `agent`; the `gh copilot` extension stopped working and Copilot
  CLI (`copilot`) is generally available; Continue CLI is discontinued; Aider is
  barely maintained (last release February 2026).
- **Homebrew 6.0** (June 2026) can upgrade auto-updating casks without
  `--greedy` when the installed bundle is older, requires third-party taps to be
  trusted before they load, and added AppImage and Linux variations to casks.
  [Release](https://brew.sh/2026/06/11/homebrew-6.0.0/),
  [tap trust](https://docs.brew.sh/Tap-Trust)
- **Other sources:** pnpm 11 isolates each global package and moved global
  binaries; npm 12 disables dependency install scripts; Podman 6 dropped Intel
  Macs; Docker Engine 29 hides untagged images without `--all`; RubyGems 4
  removed `gem query`. PkgDeck already lists Docker images with `--all` and an
  explicit format, uses none of the removed commands, and its pnpm, npm and
  RubyGems lifecycle jobs pass against current releases. Tap trust still needs a
  clear message when a cask from an untrusted tap cannot load.
- **Volta is unmaintained** and points users to mise, which moves mise up.

## Validation on a Mac

On 2026-09-27 the `macos-apps` inventory from PR #114 ran on an Apple silicon
Mac with macOS 26.6 and Homebrew 7.0.6, which has a real mix of Homebrew and
other apps. `pkd --from macos-apps list` read the 42 bundles in `/Applications`
in about two seconds without launching them. It matched the 29 casks that
leave an app link to their exact bundles, including bundles named differently
from their cask (`Code` for `visual-studio-code`).

The first run left six apps unmatched. Every one stayed unknown rather than
being guessed, as step 1 below requires, and two kinds are now matched from
evidence Homebrew already records:

| Case | Apps | Now |
| --- | --- | --- |
| Cask installs a `.pkg` | Tailscale; Google Drive with its Docs, Sheets and Slides apps | Matched. The cask's `uninstall pkgutil:` step names the receipts its package wrote; `pkgutil` lists the bundles each receipt installed |
| Renamed cask | VNC Viewer | Matched. `vnc-viewer` became `realvnc-connect-viewer`; `brew info` lists the old token, and the app link is still in the old token's Caskroom folder |
| Broken Caskroom record | LibreOffice, qBittorrent | Still unknown: no installed-version folder, so `brew info --installed` omits them |

Receipt matching uses only receipts an installed cask names and bundles the
receipt itself lists, so an app installed from the vendor's own package, with
no matching cask installed, stays unmatched. It adds about 0.2 s to a scan. The
summary for unmatched apps says ownership is unknown instead of "No Homebrew
ownership record found", which read as "not from Homebrew".

Edge cases, checked with temporary bundles in `~/Applications`:

| Case | Result |
| --- | --- |
| A second copy of a Homebrew app | Its own user-scope row with unknown ownership; the Homebrew copy stays managed. Neither claims the other |
| Malformed, unreadable or missing `Info.plist` | Listed, with an unknown version |
| A folder it may not open | Every other app is listed, and the skipped folder is reported |
| Scan time, 46 bundles | About 1.3 s once warm with receipt matching, 0.7 s of it `brew info --installed`; the first run took 1.9 s. A cold boot was not measured |

The GUI treated the skipped folder as a failed source: "Couldn't check macOS
Applications", a raw OS error and a "Turn off" button. It now shows "Some apps
couldn't be read", names the folder in plain words and offers no "Turn off".

The same Mac also exercised the packaged GUI from PR #118: the standalone
`PkgDeck.app` detected Homebrew, casks and developer managers when opened from
Finder, and a search no longer reported the Linux-only AppImage source as
failing. That was a detection gap: AppImage was the only source registered
without an availability probe, so macOS searches always queried it.

## Baseline coverage and remaining gaps

The baseline source registry contains **25 adapters**:

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

- PR #114 adds a limited macOS `.app` inventory; there is no adoption operation.
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

**Scope rule:** PkgDeck manages what a tool installs globally, for the user or
the system, not what a project pins. A new source qualifies only through its
global installs: mise's global tools (`mise use -g`), pixi's global
environments, Go binaries in `GOBIN`, .NET global tools. Project files such as
`mise.toml`, `.tool-versions`, `pixi.toml`, `package.json` or lockfiles are never
read as inventory or rewritten by an update. Tools with no global install mode
are out of scope.

Effort: S = bounded catalog/adapter change; M = adapter plus UI and lifecycle
tests; L = new ownership, transaction, or platform behavior. These are relative
sizes, not delivery estimates. P0 is foundational; P1 follows; P2 needs demand
or a dedicated design.

| Priority | Opportunity | User benefit | Effort | Recommended first scope |
| --- | --- | --- | --- | --- |
| P0 | macOS app inventory and Brew matching | Makes software outside package databases visible | M | Shipped in PR #114 and validated on a Mac; PR #118 matches `.pkg` casks by receipt and follows cask renames |
| P0 | Linux Homebrew cask compatibility | Expands an existing integration | M | Implemented in PR #118: Homebrew 6+ probe, Linux-only search, AppImage ownership, container lifecycle on both CPUs |
| P0 | mise | Runtime and developer-tool coverage across Linux/macOS; Volta's successor | M–L | Implemented: global tools only, registry search, upgrades within the configured range, remove with prune |
| P0 | AI CLI catalog using current managers | Easier discovery without duplicate backends | S–M | Implemented: npm and PyPI searches offer exact packages for 13 tools; Homebrew already finds the casks and formulae by name. npm, crates.io, RubyGems and Packagist searches now also query the registry |
| P1 | Standalone AI CLI adapters | Covers official installers outside package managers | M each | Implemented for Antigravity, Cursor, Copilot and Kiro CLIs, Amp and Factory Droid |
| P1 | Manage existing Mac apps with Homebrew | Converts manual installs into tracked installations | L | Independently verified adoption and recovery for an allowlist |
| P1 | Mac App Store via mas | Covers apps that should retain App Store ownership | M | Implemented: inventory, update checks that never download, and verified `mas update` runs; mas asks for the password itself, so updates need a terminal |
| P1 | Conda, mamba/micromamba and pixi | Scientific and data-development environments | L | Implemented: requested packages in named conda/mamba/micromamba environments (dry-run solve, verified update) and `pixi global` environments (full lifecycle, updates within the manifest spec); tested end to end in CI |
| P2 | Rustup | Toolchain coverage beyond Cargo-installed executables | S–M | Installed toolchains; `rustup check` exits 100 when updates exist |
| P2 | MacPorts | Completes another macOS package ecosystem | M | Native package lifecycle with variants preserved |
| P2 | Nix profiles | Useful cross-platform package coverage | L | User profiles only; verify the real (version 3) profile JSON first |
| P2 | rpm-ostree and bootc | Supports immutable desktops | L | Deployment inventory and pending-reboot state from both tools |
| P2 | AUR | Broader Arch application availability | L | Read-only: `pacman -Qm` plus AUR versions; no builds after the 2026 AUR attack |
| P2 | apk and XBPS | Extends distro coverage | M each | CLI-first integration and distro fixtures |
| P3 | Go binaries, .NET tools | Covers developer tools outside package managers | M | Only with reliable metadata (`go version -m`, `dotnet tool list -g`) |

Skipped: Volta (unmaintained), Deno and Yarn globals (no inventory command),
asdf, proto and aqua (covered by mise or too small), and Distrobox (containers,
not packages). [Topgrade](https://github.com/topgrade-rs/topgrade) upgrades
across managers but keeps no inventory, which is PkgDeck's difference.

### Package-manager feasibility

**mise is the strongest first addition.** Its inventory has JSON output and
records multiple tool versions and their configuration sources. Its upgrade
command exposes tool upgrades. Following the scope rule, list and update only
the tools in mise's global configuration; project configuration, pins, and
installed-but-inactive versions stay out of the inventory. A tool installed through mise's npm backend must not also be claimed
as a global npm installation. Preserve the controlling manager and configuration
path. [Inventory](https://mise.jdx.dev/cli/ls.html),
[upgrades](https://mise.jdx.dev/cli/upgrade.html).

For the initial mise adapter, make the configuration context explicit, keep
version constraints, and avoid `--bump` unless configuration edits are separately
reviewed. Preview with `--dry-run`; preserve old installations with `--no-prune`
until removal has its own reviewed plan. Tool updates must not silently become
configuration rewrites or removal of versions used elsewhere.
[mise upgrade options](https://mise.jdx.dev/cli/upgrade.html).

**Conda is valuable but requires environment transactions.** Its update command
supports JSON and dry-run output. An update can change several packages, so show
the solver's full plan and revalidate it before execution. Use canonical prefix
paths as scope; retain channels and build identities, and never silently update
every environment. Start with Conda; test mamba and micromamba separately before
advertising compatibility. Mamba 2 and micromamba share one codebase, so one
adapter can switch binaries. pixi fits beside them: `pixi global list --json`
and `pixi global update` cover its global tools. Anaconda's licensing moves many
users to Miniforge, so do not assume the `defaults` channel.
[Conda update](https://docs.conda.io/projects/conda/en/stable/commands/update.html),
[pixi global](https://pixi.prefix.dev/latest/reference/cli/pixi/global/update/).

**mas fills a different need from casks.** It inventories and updates App Store
apps, with Apple Account and privilege requirements for mutations. Its accurate
outdated check initiates a download to read metadata; do not describe that as a
purely local read. Start with inventory and clearly labeled update checking;
let Apple handle authentication. mas runs sudo itself, and sudo only asks
for a password in a terminal, so GUI updates explain that instead of prompting.
mas 7 added `--json` to `list`, `outdated`,
`lookup` and `search`, and mas 4 fixed installs on macOS 26.1; installs and
upgrades need root. [mas documentation](https://github.com/mas-cli/mas),
[releases](https://github.com/mas-cli/mas/releases).

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
ordinary live RPM updates. Fedora's image-mode work adds DNF and bootc next to
rpm-ostree rather than replacing it yet, so read both status outputs.
[Upstream handbook](https://github.com/coreos/rpm-ostree/blob/main/docs/administrator-handbook.md),
[Fedora change](https://fedoraproject.org/wiki/Changes/DNFAndBootcInImageModeFedora).

**AUR** requires build-recipe review and helper-specific behavior. Integrate one
installed helper first, retain its review/diff step, and distinguish AUR origin
from Pacman's installed-package ownership. Avoid duplicate rows for the same
installed package. In June 2026 malicious commits reached about 1,500 AUR
packages and new AUR accounts were blocked, so start read-only: list foreign
packages with `pacman -Qm`, compare versions through the AUR RPC, and leave builds
to the user's helper. [ArchWiki AUR helpers](https://wiki.archlinux.org/title/AUR_helpers),
[SecurityWeek](https://www.securityweek.com/atomic-arch-supply-chain-attack-hits-1500-aur-packages/).

Windows managers such as WinGet, Scoop, and Chocolatey should be a separate
platform initiative. Current Unix host execution and Linux/macOS distribution
support make them more than additional command adapters. Similarly, defer
additional runtime managers until mise establishes multi-version semantics.

## AI CLIs: expand coverage without multiplying sources

| Tool | Status (2026-09) | Install routes | Proposed handling |
| --- | --- | --- | --- |
| Claude Code, Codex, Grok, OpenCode | Active | Official installers, npm, casks (`claude-code`, `codex`), OpenCode tap | Keep the standalone adapters; Claude Code now also ships apt, dnf and apk repositories, which the native managers own |
| Antigravity CLI (`agy`) | Active; replaces Gemini CLI | Installer to `~/.local/bin`, cask `antigravity-cli` | Standalone adapter; verify how it updates, since only background self-update is documented |
| Gemini CLI | Deprecated for consumer accounts | npm `@google/gemini-cli`, deprecated formula `gemini-cli` | No new work; point existing installs to Antigravity CLI |
| GitHub Copilot CLI (`copilot`) | Generally available | npm `@github/copilot`, cask `copilot-cli`, installer | Catalog npm and cask; standalone adapter using `copilot update` |
| Cursor CLI (`agent`) | Active; `cursor-agent` is an alias | Installer to `~/.local/bin`, cask `cursor-cli` | Standalone adapter with `agent update`; see the `agent` clash below |
| Kiro CLI (`kiro-cli`) | Replaces Amazon Q Developer CLI | Installer, cask `kiro-cli`, Linux `.deb` | Standalone adapter with `kiro-cli update`; the `amazon-q` cask now points here |
| Amp (`amp`) | Active | Installer to `~/.amp/bin`, npm `@sourcegraph/amp` | Catalog npm; standalone adapter with `amp update`. The `amp` formula is an unrelated editor |
| Factory Droid (`droid`) | Active | Installer, cask `droid`, npm `@factory/cli` | Catalog cask and npm; standalone adapter later |
| Qwen Code (`qwen`) | Active | npm `@qwen-code/qwen-code`, formula `qwen-code` | Catalog only |
| Crush (`crush`) | Active | Charm tap and repositories, npm `@charmland/crush`, AUR | Catalog only |
| goose (`goose`) | Active; now under the Linux Foundation | Installer, formula `block-goose-cli` | Catalog only. The `goose` formula is an unrelated migration tool |
| Warp Agent CLI (`warp`) | Launched August 2026 | Installer, cask `warp-agent-cli` | Catalog only; the `warp` cask is the terminal app |
| Cline CLI (`cline`) | Active; Homebrew formula deprecated | npm `cline` | Not in the catalog: a malicious 2.3.0 was published in February 2026, and catalog offers install `latest` without a reviewed version |
| Kimi Code, Mistral Vibe | Active | PyPI and formulae `kimi-code`, `mistral-vibe` | Catalog only |
| Aider | Barely maintained | PyPI `aider-chat`, formula `aider` | Existing pip/pipx/uv/Homebrew sources are enough; no new work |
| Continue CLI | Discontinued | npm `@continuedev/cli` | None |
| Ollama | Active | Installer, formula `ollama`, cask `ollama-app` | Separate desktop app, CLI/service, and model inventory; defer native Linux service migration |

“Catalog” means mapping a product to exact package IDs in the managers PkgDeck
already supports. It is an architectural assessment, not a claim that these
applications have passed PkgDeck end-to-end tests; add targeted fixtures and
real lifecycle smoke tests before advertising support.

**Never match an AI tool by name.** Homebrew's `amp`, `goose` and `grok`
formulae and its `gemini` cask are unrelated tools. Use exact package IDs and
installer layouts.

**Two tools install an `agent` command.** Cursor's installer puts `agent` in
`~/.local/bin`, and the Grok installer can create an `agent` symlink there too,
so installing one can take over the other's command. A Cursor adapter must
resolve the executable to Cursor's versions directory
(`~/.local/share/cursor-agent/versions/`) before claiming it, and must not
treat a bare `agent` on `PATH` as Cursor.
[Cursor CLI installation](https://cursor.com/docs/cli/installation),
[Grok Build](https://docs.x.ai/build/overview)

Ollama's native Linux installation involves libraries and potentially a system
service; its documented update reruns installation. On macOS, distinguish the
`ollama` formula from the `ollama-app` cask. Model downloads should have their
own size and removal semantics, not be confused with updates to the program.
[Linux installation](https://docs.ollama.com/linux).

Official evidence: [Copilot CLI](https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli),
[Kiro CLI](https://kiro.dev/docs/reference/cli-commands/),
[Amp](https://ampcode.com/docs/cli),
[Factory Droid](https://docs.factory.ai/cli/getting-started/overview),
[Warp Agent CLI](https://docs.warp.dev/agents/cli/quickstart/),
[Continue](https://github.com/continuedev/continue).

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

Pick the evaluation set from Homebrew's install analytics (365 days to
2026-09-27) rather than guesses. The most-installed app casks are, in order:
docker-desktop, visual-studio-code, google-chrome, ghostty, libreoffice,
iterm2, claude, obsidian, firefox, orbstack, warp, stats, postman,
dbeaver-community, cursor, vlc, android-studio, raycast, wine-stable and
rectangle. All of them auto-update except libreoffice and wine-stable.
Their presence in Homebrew does **not** establish safe adoption of every
existing installation.
[Cask install analytics](https://formulae.brew.sh/api/analytics/cask-install/365d.json)

| App | Cask | Why it is in the set |
| --- | --- | --- |
| Visual Studio Code | [visual-studio-code](https://formulae.brew.sh/cask/visual-studio-code) | Popular, self-updating; check `code` launcher conflicts |
| Google Chrome | [google-chrome](https://formulae.brew.sh/cask/google-chrome) | Popular, self-updating; validate managed-install interactions |
| Obsidian | [obsidian](https://formulae.brew.sh/cask/obsidian) | Also a Linux AppImage cask; preserve vaults and settings |
| Cursor, Warp | [cursor](https://formulae.brew.sh/cask/cursor), [warp](https://formulae.brew.sh/cask/warp) | Also Linux AppImage casks |
| LibreOffice | [libreoffice](https://formulae.brew.sh/cask/libreoffice) | Does not self-update, so Homebrew owns its updates |
| Docker Desktop | [docker-desktop](https://formulae.brew.sh/cask/docker-desktop) | Heavy install and uninstall steps; not an adoption candidate |
| Firefox | [firefox](https://formulae.brew.sh/cask/firefox) | Distinguish stable, ESR, beta and language |
| A cask from a third-party tap | e.g. `steipete/tap/codexbar` | Exercises Homebrew 6 tap trust |

Avoid building a universal updater for arbitrary `.app`, DMG, PKG, ZIP, or
tarball installations. A supported package manager or a verified upstream
updater should own mutations. Sparkle is an app update framework, not proof
that every bundled application offers a uniform external update command.
[Sparkle documentation](https://sparkle-project.org/documentation/).

## Moving standalone Mac apps to Homebrew

**Yes, support this, as an explicit ownership-transfer feature.** Cask
availability alone is insufficient. The manual describes `--adopt` as adopting
identical artifacts and disallows combining it with `--force`.
[Homebrew manual](https://docs.brew.sh/Manpage).

**Do not treat that description as an equality guarantee.** In Homebrew 7.0.6,
the app-artifact implementation skips its equality comparison for
`auto_updates` casks. Otherwise, when both bundles provide version metadata,
it compares the short and build versions; recursive file comparison is a
fallback. Successful adoption therefore does not prove equal contents or
publisher identity. The proposed VS Code, Firefox, and Obsidian casks are
self-updating. PkgDeck must enforce its own documented identity/content policy
before invoking Homebrew; the first MVP can deliberately require stricter
artifact equality. [Versioned adoption implementation](https://github.com/Homebrew/brew/blob/7.0.6/Library/Homebrew/cask/artifact/moved.rb),
[VS Code cask](https://formulae.brew.sh/cask/visual-studio-code),
[Firefox cask](https://formulae.brew.sh/cask/firefox),
[Obsidian cask](https://formulae.brew.sh/cask/obsidian).

Adoption is also a real installation transaction. App installation adjusts
permissions/group ownership, and a later artifact failure runs uninstall phases
for earlier artifacts before purging versioned files. That creates a recovery
risk even after the app itself was successfully adopted; a nonzero exit alone
does not prove the original bundle is untouched.
[App installation](https://github.com/Homebrew/brew/blob/7.0.6/Library/Homebrew/cask/artifact/app.rb),
[Installer failure handling](https://github.com/Homebrew/brew/blob/7.0.6/Library/Homebrew/cask/installer.rb).

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
4. **Prepare recovery before adoption.** Preserve the original bundle outside
   Homebrew's staging/cleanup directories, including permissions and extended
   attributes. Record the cask recipe, all artifact destinations, and recovery
   steps. Check launcher conflicts and reject unreviewed install hooks. Recovery
   is required for adoption as well as replacement.
5. **Adopt only after PkgDeck's checks pass.** Run a narrowly authorized
   `brew install --cask --adopt TOKEN` operation and verify the resulting Brew
   record and artifact path. On failure, inspect actual state, restore when
   necessary, and explain the outcome. Do not assume Homebrew rejected the
   transaction before making any changes.
6. **Handle a different version separately.** Offer a reviewed replacement
   workflow only for supported simple app bundles: stage the old bundle in a
   recovery location, install through Brew, verify, and retain recovery until
   accepted. Do not fall back automatically from failed adoption to replacement.
7. **Verify ownership.** Refresh both inventories, check launcher resolution,
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
`app_image` artifacts and portable binaries. Support started as preliminary in
Homebrew 4.5 and gained AppImage artifacts and Linux variations in 6.0. On
2026-09-27, 2,873 of 7,764 casks supported Linux: 2,592 fonts, 214 AppImages and
the rest binaries. `app`, `pkg` and `suite` artifacts stay macOS-only. The two
most-installed casks overall, `claude-code` and `codex`, are Linux-capable
binary casks. Obsidian's cask currently includes
Linux AppImages. PkgDeck's macOS-only registration therefore excludes a real
upstream capability. Linux artifacts are also documented in the **7.0.6 release
source**, so this is not only an unreleased-main observation.
[Versioned cask cookbook](https://github.com/Homebrew/brew/blob/7.0.6/docs/Cask-Cookbook.md),
[acceptable casks](https://docs.brew.sh/Acceptable-Casks),
[Obsidian cask](https://formulae.brew.sh/cask/obsidian).

**Implemented in PR #118.** Checked against Homebrew 7.0.6 on Linux:

- On Linux, `brew info --json=v2 --cask` describes each cask for Linux. A
  Mac-only cask declares `depends_on: macos` (Firefox, Rectangle, Chrome) or
  keeps `app` or `pkg` artifacts; a Linux cask ships `app_image`, `binary` or
  `font` artifacts (Obsidian, Cursor, Warp, `claude-code`, `codex`). Search
  drops Mac-only casks; installed casks always stay listed.
- The source registers on Linux and requires Homebrew 6.0 or later, the first
  release with AppImage artifacts and Linux variations.
- Homebrew moves an AppImage into `~/Applications` and leaves a symlink to it
  in the cask's installed version folder. The AppImage source skips files those
  symlinks point to, so it no longer offers to update or remove an AppImage a
  cask owns (for example after the AppImage added its own desktop entry).
- Verified in a Linux arm64 container: search, details, install, remove, and
  the AppImage exclusion with real casks (Obsidian, `1password-cli`). The
  container lifecycle suite now runs a local cask through detect, search,
  details, install, update, upgrade and remove on both Linux CPUs, from the CLI
  and the GUI.

## Suggested implementation sequence and acceptance criteria

1. **Validate the foundation:** PR #114 now runs on an actual Mac (see
   [Validation on a Mac](#validation-on-a-mac)), including active-prefix
   ownership, broken Caskroom records, duplicate copies, unreadable bundles and
   scan timing, and matches `.pkg` casks and renamed casks. Still open: cold-boot timing. Missing ownership evidence must
   stay unknown. Add publisher/channel/architecture verification before migration.
   Introduce the AI catalog using existing package routes; prototype Linux casks
   and mise inventory separately. These are separate work items, not one P0
   delivery commitment.
2. **Bounded additions:** mise global-tool management, standalone adapters for
   Antigravity, Cursor (resolving the shared `agent` name), Copilot and Kiro
   CLIs, and mas inventory/selected updates.
3. **Mac adoption MVP:** three curated app families (VS Code, Firefox stable,
   Obsidian), PkgDeck-enforced artifact checks, exact preconditions, and postchecks.
   Require tested recovery before enabling either adoption or replacement.
4. **Broader ecosystems:** Conda-family and pixi environment plans, then
   Rustup/MacPorts; pursue Nix, immutable systems, read-only AUR, apk, and XBPS
   according to user demand.

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
- Inject an adoption failure after the app step (for example, a launcher
  conflict) and verify the original bundle can be restored independently of
  Homebrew's cleanup. Verify permissions and extended attributes too.
- Replacement interruption has a recorded, tested recovery path; app data stays
  unchanged in success and failure cases.
- Intel and Apple Silicon Brew prefixes, custom app directories, launcher
  conflicts, running apps, and multiple installed versions are covered.
- New managers preserve environment pins, configuration sources, and full
  dependency plans; “Update all” does not escape the selected scope.
- Absent sources remain cheap to detect. Benchmark startup and background
  checks so new adapters do not undo the recent performance work.

The inventory in PR #114 now has native macOS CI coverage on both architectures.
The delivery gate is a passing complete matrix for the reviewed commit. Adoption
remains blocked on independent identity/content checks and tested recovery;
the Linux-cask compatibility spike can proceed as a separate work item.
