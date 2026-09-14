# PkgDeck

A unified package manager interface for Linux, built with Rust and Qt/Kirigami.

PkgDeck is built from one codebase with two frontends:

- `pkgdeck` — GUI frontend using Qt/Kirigami.
- `pkd` — terminal frontend without Qt:
  - `pkd` opens the interactive TUI.
  - `pkd <command>` runs a normal CLI command.

Both use the shared `pkgdeck-core` Rust library.

## Preview

Soft design references (including planned backends). See the [implemented GUI](docs/gui.md)
and [terminal guide](docs/tui.md) for the current interface and supported behavior.

![PkgDeck GUI](docs/screenshots/pkgdeck-gui.png)

![pkd TUI](docs/screenshots/pkd-tui.png)

![pkd CLI](docs/screenshots/pkd-cli.png)

## Planned initial support

- **System:** APT, DNF, Pacman, Zypper.
- **Applications:** Flatpak, Snap, Homebrew (Linux formulae), AppImage.
- **Development:** Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems.

Development package managers will initially focus on user-installed command-line tools rather than project dependencies. pip support will be restricted to explicitly selected virtual environments.

AppImage support will initially cover importing local Type 2 AppImages, desktop integration, launching, and removal.

## Architecture

```text
pkgdeck-core   Shared package-management logic
pkgdeck        GUI frontend, with Qt/Kirigami
pkd            Terminal frontend, without Qt
               ├── TUI
               └── CLI
```

```text
pkgdeck ─┐
         ├── pkgdeck-core
pkd ─────┘
```

`pkgdeck-core` must remain independent of Qt. Package-manager logic lives in the core and is shared by both frontends.

Each backend exposes the operations it actually supports, such as detection, search, package details, installed packages, install, remove, update, and upgrade. Unsupported operations are reported explicitly.

## Terminal interface

Running `pkd` without arguments opens the interactive TUI:

```sh
pkd
```

APT and Linux Homebrew CLI commands are implemented:

```sh
pkd search neovim
pkd info neovim
pkd install neovim
pkd install neovim --from homebrew
pkd remove neovim
pkd list
pkd sources
pkd update
pkd upgrade
```

Commands support machine-readable `--json` output. Writes require `--yes` in
non-interactive or JSON mode. See the [CLI contract](docs/cli.md) for source
selection, authorization, exit codes, prerequisites, and examples.

If stdin or stdout is not attached to a terminal, `pkd` without arguments must not attempt to start the TUI.

## Implementation plan

The screenshots are soft references for information hierarchy and interaction:
source/version columns, selection-linked details, clear actions, and readable
progress. Layouts must adapt to the window or terminal. Theme colors, icons,
available packages, and progress indicators must reflect actual platform and
backend capabilities rather than reproduce the mockups literally.

### 1. Workspace and build pipeline — implemented

Implemented in [PR #3](https://github.com/astrovm/PkgDeck/pull/3).
[GitHub Actions](https://github.com/astrovm/PkgDeck/actions/workflows/ci.yml)
verifies the complete x86_64/aarch64 build, test, coverage, and package matrix.

- Create the `pkgdeck-core`, `pkd`, and `pkgdeck` Cargo workspace.
- Keep Qt entirely within `pkgdeck`; build `pkd` independently without Qt.
- Pin Rust, Qt 6, Kirigami, CXX-Qt, and the terminal libraries.
- Add minimal Qt/Kirigami and Ratatui/Crossterm application shells.
- Build both executables and Flatpak, AppImage, and Snap packages in GitHub
  Actions on native x86_64 and aarch64 runners.
- Enforce formatting, linting, behavioral smoke tests, and at least 95% Rust
  line coverage. PR builds do not publish releases.

**Acceptance:** CI builds both entry points and all three formats for both
architectures, and the terminal build has no Qt dependency.

### 2. Host execution and authorization — implemented

The shared host boundary and `pkd doctor` are implemented. Native/AppImage host
reads are enabled; Flatpak and Snap host operations are explicitly disabled.
The APT authorization prototype passes synthetic tests and a disposable Ubuntu
x86_64 VM test. See [execution behavior and format gates](docs/host-execution.md).

- Define native, Flatpak, AppImage, and Snap host execution paths. Resolve package
  managers and user environments on the host, outside packaging runtimes.
- Prototype backend detection and authorized APT operations in disposable VMs.
- Keep frontends unprivileged. Use polkit or existing host authorization only
  where required; never elevate Homebrew or user-scoped development tools.
- Coordinate writes with native manager locks. Define authentication failure,
  lock contention, cancellation, and interrupted-operation behavior.
- Validate `flatpak-spawn --host` and its required D-Bus permission before enabling
  host operations. Validate AppImage and Snap host access separately.

**Acceptance:** each format has a tested execution path or a documented,
explicitly disabled capability. Snap confinement is an early feasibility gate.

### 3. Shared engine — implemented

The core now has package identities, a backend contract, explicit selection,
partial-result handling, and operation progress events. Synthetic backends verify
the full engine lifecycle. See the [shared engine contract](docs/shared-engine.md).

- Model package identity by backend, package identifier, architecture, and
  installation scope or environment.
- Define backend capabilities, package details, installed state, update
  availability, typed errors, and operation progress events.
- Prefer documented APIs and structured output. Execute argument arrays rather
  than shell strings; report unsupported operations explicitly.
- Separate metadata refresh (`update`) from installed-package upgrades (`upgrade`).
- Resolve ambiguous names explicitly. Matching names across sources do not prove
  that packages represent the same application.

**Acceptance:** synthetic backends exercise discovery, selection, operations,
partial failures, and progress without frontend-specific package-manager logic.

### 4. APT and Homebrew lifecycle through the CLI — implemented

APT and Homebrew adapters now use the shared engine and host boundary. The CLI
provides all eight lifecycle capabilities, exact source/architecture selection,
JSON results, and explicit confirmation. See the [CLI contract](docs/cli.md).

- Implement detection, search, details, installed listing, refresh, install,
  upgrade, and removal for APT and Linux Homebrew formulae.
- Add the documented CLI commands, source selection, consistent exit codes,
  machine-readable JSON, and explicit non-interactive confirmation behavior.
- Keep diagnostics and progress separate from machine-readable output.
- Preserve the no-argument terminal check; never launch the TUI when stdin or
  stdout is not a terminal.

**Acceptance:** synthetic versioned packages complete detect → search/details →
install → detect update → upgrade → remove. Verify final state using the real
underlying managers in isolated environments.

### 5. Interactive TUI — implemented

Search, installed packages, updates, sources, exact-identity confirmations, and
background operations are implemented. See the [TUI guide](docs/tui.md).
PTY tests exercise interaction and terminal restoration; disposable containers
verify the APT/Homebrew lifecycle through the TUI against native package state.

- Add search, results, package details, installed packages, updates, and sources.
- Add confirmation, progress, cancellation, errors, and authentication failures.
- Use the reference's results-above-details layout and shortcut footer; adapt to
  narrow terminals and provide text fallbacks for icons.

**Acceptance:** the APT/Homebrew lifecycle works without Qt or a graphical session;
pseudo-terminal tests cover keyboard navigation, resize, and terminal restoration.

### 6. GUI — implemented

The Qt/Kirigami browser now uses an asynchronous CXX-Qt facade over the shared
engine. It provides source-aware package actions, persistent settings, responsive
views, and safe cancellation. See the [GUI guide](docs/gui.md) for shortcuts,
authorization, and Qt Quick/private-display/packaged-bundle verification.

- Connect the engine through a thin CXX-Qt bridge with asynchronous operations.
- Build sidebar navigation, search/results, selection-linked details, and
  source-aware actions. Add Installed, Updates, Sources, Settings, Help/About,
  and Discover backed by available data.
- Use system themes, dark mode, accessible controls, keyboard navigation, and
  virtualized lists or `TableView`.
- Include loading, empty, unavailable-backend, authorization, and failure states.

**Acceptance:** GUI users complete the same APT/Homebrew lifecycle, verified with
Qt Quick tests and packaged-app end-to-end tests.

### Frontend polish — implemented

The GUI, TUI, and CLI now share the references' results-first layout and clear
source identity. The GUI adds light/dark/system appearance, contextual actions,
and compact navigation. The TUI adds styled tables and readable operation
confirmations. Human CLI output uses aligned tables and labeled details; `--json`
retains its versioned contract. Terminal color is omitted when output is piped or
`NO_COLOR` is set (CLI).

GUI detail snapshots avoid repeated native queries and are invalidated on reload
or writes. The TUI no longer redraws continuously while idle. Local checks cover
keyboard flows, narrow layouts, cache reuse/invalidation, human output, and the
existing synthetic/native lifecycle suites.

### 7. Additional backends — planned

Add each wave only after its advertised capabilities pass integration tests.

| Wave | Backends | Initial scope |
| --- | --- | --- |
| 2 | AppImage, Flatpak | Local Type 2 imports; Flatpak user/system installations. |
| 3 | DNF, Pacman, Zypper, Snap | Distro-specific operations and Snap lifecycle. |
| 4 | Cargo, npm, pnpm, Bun | User-installed command-line tools. |
| 5 | pip, pipx, uv, Composer, RubyGems | Explicit environments and isolated user tools. |

For AppImages, manage only PkgDeck-owned files and desktop entries; never execute
an imported AppImage merely to inspect metadata. Restrict pip to explicitly
selected virtual environments.

**Acceptance:** each enabled capability has synthetic success/failure fixtures
and lifecycle tests that confirm state through its underlying manager.

### 8. Release automation and distribution validation — planned

- Validate both entry points, desktop integration, host access, authorization,
  and architecture-specific limitations in each installed package format.
- Publish only the exact artifacts built and tested by GitHub Actions.
- Dispatch Flatpak publication to `astrovm/flatpak` without rebuilding, and
  automate Snapcraft publication.
- Gate tagged releases on the complete required test/package matrix for both
  architectures and at least 95% Rust line coverage.

**Acceptance:** automated publication and verification of the CI-tested artifacts;
no local or manually prepared release builds.

## Development

Start with `scripts/verify.sh fast`. Run `scripts/setup-dev.sh` once to prepare
the persistent SDK, then `scripts/verify.sh full` for workspace coverage and release
builds. `scripts/verify.sh vm` runs isolated native acceptance checks. Local work
and CI use these same scripts; see [development tooling](docs/development.md).
Use `scripts/verify.sh full --engine podman` for the pinned rootless build
environment and `scripts/verify.sh containers --engine podman` for disposable APT
and Homebrew tests. VM dependencies are cached separately from test overlays.

The CLI supports APT and Homebrew package operations and `pkd doctor` diagnostics.
The core contains their adapters, the shared engine, and the host authorization
boundary. Both the interactive TUI and GUI use the same engine. Additional
backends and release publication remain later steps.

Pinned baseline:

| Component | Version |
| --- | --- |
| Rust | 1.98.1 |
| Python (CI) | 3.14.7 |
| Qt | 6.11.2 |
| Kirigami / Extra CMake Modules | 6.30.0 |
| CXX-Qt | 0.10.0 |
| CXX / CXX generator | 1.0.202 / 0.7.202 |
| Ratatui / Crossterm | 0.30.2 / 0.29.0 |
| Clap | 4.6.6 |
| Serde / serde_json | 1.0.229 / 1.0.151 |
| signal-hook (CLI) | 0.4.4 |
| CMake / aqtinstall | 4.4.3 / 3.3.0 |
| cargo-llvm-cov | 0.9.1 |

`Cargo.lock` is committed. Use the pinned toolchain and `--locked` in builds.
The CXX generator is pinned alongside CXX because they must use the same bridge ABI.
Use Python 3.14 for aqtinstall: Python 3.12 can misidentify the Qt 6.11.2 ARM
QtTools archive as ZIP and fail extraction with aqtinstall 3.3.

Build and test the terminal frontend without installing Qt:

```sh
cargo build --locked -p pkd
cargo test --locked -p pkgdeck-core -p pkd
cargo run --locked -p pkd -- --help
cargo run --locked -p pkd -- doctor
cargo run --locked -p pkd
```

The last command opens the package browser in a terminal. See the
[TUI shortcuts and authorization guide](docs/tui.md). Use `q` to exit when idle.
With piped input/output, no-argument execution fails with exit code 2.
Python 3 is required for the pseudo-terminal integration tests.

For the GUI, install the pinned Qt SDK (including ShaderTools and ImageFormats),
a C++ compiler, the pinned CMake (Kirigami requires at least 3.29), Ninja, lld, and the platform development libraries listed
in `.github/workflows/ci.yml`. Build Kirigami into a local SDK prefix:

```sh
export QT_ROOT_DIR=/absolute/path/to/Qt/6.11.2/gcc_64
export PKGDECK_SDK_PREFIX="$PWD/build/kde"
export PATH="$QT_ROOT_DIR/bin:$PATH"
export CMAKE_PREFIX_PATH="$QT_ROOT_DIR"
scripts/build-kirigami.sh
export LD_LIBRARY_PATH="$PKGDECK_SDK_PREFIX/lib:$QT_ROOT_DIR/lib"
export QML_IMPORT_PATH="$PKGDECK_SDK_PREFIX/qml:$QT_ROOT_DIR/qml"
cargo build --workspace --locked
cargo test --workspace --locked
cargo run --locked -p pkgdeck
```

On aarch64, use the `gcc_arm64` Qt SDK directory. GUI tests use Qt's offscreen
platform and software rendering; they load the real Kirigami window and exit
through `--smoke-test`. The CI workflow is the authoritative build recipe.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo install cargo-llvm-cov --version 0.9.1 --locked
cargo llvm-cov --workspace --include-build-script --locked --fail-under-lines 95
```

For local package staging, build the release workspace, run
`python3 scripts/bundle.py`, then `scripts/package.sh appimage`, `flatpak`, or
`snap` with the corresponding tools/runtime installed. The AppImage path requires
`APPIMAGETOOL` pointing to appimagetool 1.9.1. All formats contain both executables.

Initial CI checks cover extracted Snap contents and entry points. Installed Snap
confinement, privileged host operations, FUSE-based AppImage launching, release
signing, and publication are later acceptance gates; foundation artifacts are
not production-ready package managers. The Flatpak build uses the KDE 6.11 runtime
and bundles the pinned Qt/Kirigami libraries; Flathub supplies runtime dependencies
only, not PkgDeck distribution.

## Packaging and releases

Package `pkgdeck` and `pkd` together in every distribution format. There is no separate CLI or TUI release.

All release formats must support **x86_64** and **aarch64**. Architecture-specific backend limitations must be detected and documented rather than silently falling back.

The planned application ID is `io.github.astrovm.PkgDeck`.

| Format   | GUI                                     | Terminal interface                                                |
| -------- | --------------------------------------- | ----------------------------------------------------------------- |
| Flatpak  | `flatpak run io.github.astrovm.PkgDeck` | `flatpak run --command=pkd io.github.astrovm.PkgDeck [arguments]` |
| AppImage | `./PkgDeck.AppImage`                    | `./PkgDeck.AppImage --cli [arguments]`                            |
| Snap     | `pkgdeck`                               | `pkgdeck.pkd [arguments]`                                         |

With no terminal arguments, the `pkd` entry point opens the TUI. With a subcommand, it behaves as a normal CLI.

### Flatpak

Build the Flatpak in the PkgDeck GitHub Actions workflow using the KDE runtime. Distribute it through https://github.com/astrovm/flatpak, not Flathub.

Release flow:

1. Build and test `x86_64` and `aarch64` `.flatpak` bundles in PkgDeck CI.
2. Attach the exact tested bundles to the tagged GitHub release.
3. Dispatch publication to `astrovm/flatpak`.
4. Let the Flatpak repository import, sign, publish, and verify the bundles without rebuilding PkgDeck.

### AppImage

Build `x86_64` and `aarch64` AppImages in GitHub Actions with the required Qt/Kirigami libraries and plugins. `AppRun` launches `pkgdeck` by default and dispatches `--cli` to the bundled `pkd` executable.

### Snap

Build and test `x86_64` and `aarch64` Snaps in GitHub Actions and publish release builds through [Snapcraft][snapcraft]. Validate confinement, host package-manager access, authorization, and both bundled entry points before marking Snap support production-ready.

## CI

All builds run in GitHub Actions for both **x86_64** and **aarch64**, including:

- `pkgdeck`
- `pkd`
- backend test fixtures
- Flatpak
- AppImage
- Snap

Pull requests run formatting, linting, unit tests, affected backend tests, coverage, and builds of all three package formats for both architectures without publishing.

Rust code must maintain at least **95% line coverage**, measured in CI with `cargo llvm-cov`. Generated bindings, vendored code, and other non-project generated sources may be excluded from the coverage calculation. CI must fail if coverage drops below 95%.

Tagged releases run the complete test and packaging matrix. Only artifacts produced and tested by CI are published; releases must not depend on local builds or manually prepared artifacts.

Flatpak publication to `astrovm/flatpak` and Snap publication to Snapcraft must also be triggered from GitHub Actions.

## Testing

Use real package-manager commands, synthetic versioned packages, and isolated environments.

| Area                     | Test environment                                                                                          |
| ------------------------ | --------------------------------------------------------------------------------------------------------- |
| APT, DNF, Pacman, Zypper | Separate distro containers with pinned images; VMs for privileged desktop workflows.                      |
| Homebrew                 | Disposable non-root Linux environment using the standard Linux Homebrew prefix and a controlled test tap. |
| Flatpak                  | Disposable VM with a local test repository and user/system installation tests.                            |
| Snap                     | Ubuntu VM with `snapd`, plus tests of the Snapcraft-published package.                                    |
| AppImage                 | CI-built Type 2 fixtures; extraction-based tests in containers and FUSE launch in a VM.                   |
| Development managers     | Disposable users, homes, prefixes, virtual environments, and controlled test registries.                  |
| GUI                      | Qt Quick Test plus end-to-end packaged-app tests.                                                         |
| TUI                      | Terminal interaction tests in a pseudo-terminal, including resize, keyboard navigation, and cancellation. |
| CLI                      | Tests without a display server, including scripting, JSON output, exit codes, and non-interactive paths.  |
| Architectures            | Build and run relevant tests on both x86_64 and aarch64.                                                  |
| Coverage                 | `cargo llvm-cov` with a minimum 95% Rust line-coverage threshold.                                         |

For every advertised capability, verify success and failure paths and confirm state using the underlying package manager rather than only PkgDeck output.

A release is considered supported only when its required tests pass on both **x86_64** and **aarch64**, and CI reports at least **95% Rust line coverage**.

## Distribution

Both entry points are built together and included in each CI-produced package:

- **Flatpak:** https://github.com/astrovm/flatpak, served from [flatpak.4st.li][flatpak-site], not Flathub.
- **AppImage:** GitHub Releases.
- **Snap:** [Snapcraft][snapcraft].

[rust-kirigami]: https://develop.kde.org/docs/getting-started/rust/
[flatpak-spawn]: https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn
[qt-tableview]: https://doc.qt.io/qt-6/qml-qtquick-tableview.html
[flatpak-repository]: https://github.com/astrovm/flatpak
[flatpak-site]: https://flatpak.4st.li/
[snapcraft]: https://snapcraft.io/
