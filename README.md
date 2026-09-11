# PkgDeck

A unified package manager interface for Linux, built with Rust and Qt/Kirigami, with the `pkgdeck` GUI and the `pkd` CLI built together from one codebase.

## Planned initial support

- **System:** APT, DNF, Pacman, Zypper.
- **Applications:** Flatpak, Snap, Homebrew (Linux formulae), AppImage.
- **Development:** Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems.

Development support will initially focus on user-installed command-line tools, with pip restricted to explicitly selected virtual environments.

AppImage support will initially cover importing local Type 2 AppImages, desktop integration, launching, and removal. Automatic AppImage updates are outside the initial scope.

## Implementation plan

The following phases describe planned work, not implemented features.

### 1. Set up the project

- Create a Cargo workspace with a shared `pkgdeck-core` library and thin `pkgdeck` GUI and `pkd` CLI entry points. Build both together, keep the core independent of Qt, and keep all package-management logic shared. The `pkd` entry point is CLI-only.
- Connect Rust to Qt 6/QML and Kirigami through CXX-Qt, following [KDE's Rust integration guide][rust-kirigami]. Use CMake for application builds and installation.
- Pin a compatible Rust/Qt/Kirigami toolchain and commit the dependency lockfile. Configure GitHub Actions to build both executables, run formatting, linting, and unit tests, and build all distribution formats from a clean checkout.
- Build minimal Flatpak, AppImage, and Snap packages in GitHub Actions early. Prove host package-manager detection and authorization from each CI-built format before expanding backend coverage.

### 2. Build the shared engine and execution layer

- Define backend capabilities for detection, search, details, installed packages, update checks, installation, upgrades, and removal. Expose unsupported operations explicitly.
- Identify packages by backend, repository, package ID, architecture, and installation scope. Group equivalent applications only with verified mappings, never by matching names alone.
- Prefer documented APIs or structured output. Isolate and test any required text parsers. Detect manager versions and only enable compatible backends on the host.
- Execute commands with argument arrays, not shell strings. Capture progress, exit codes, and errors; keep package operations off the GUI thread.
- Coordinate writes across the GUI and CLI per package database, respect native locks, and allow cancellation only at backend-safe points. Report partial failures without promising cross-manager rollback.
- Keep the GUI unprivileged. Use narrowly scoped, polkit-authorized host operations with validated requests and trusted executable paths. Never elevate Homebrew or user-scoped development tools.
- Prototype Flatpak host execution using [flatpak-spawn --host][flatpak-spawn] with explicit D-Bus permission. Document its broad host access, verify the host environment, and resolve any host-helper installation requirements before enabling writes.
- Validate Snap confinement, host environment access, and authorization before enabling host package operations.

### 3. Deliver the first complete workflow

- Implement APT and Homebrew first, with controlled test packages covering installation, inspection, upgrades, and removal.
- Add `pkd search`, `info`, `list`, `sources`, `install`, `remove`, `update`, and `upgrade`. CLI commands should support `--from` where applicable, machine-readable `--json` output, and consistent exit codes. `update` refreshes metadata/checks availability; `upgrade` applies changes. Keep GUI launch separate from CLI commands.
- Require an explicit source when a package is ambiguous. Preview changes where supported and confirm destructive operations. Preserve each manager's transaction rules rather than forcing identical upgrade behavior.
- Connect GUI search, package details, and installed-package views to the same engine. Complete one real lifecycle through both interfaces before adding more backends.

### 4. Expand backend coverage

Add each group only after its supported operations pass integration tests:

| Order | Backends | Initial scope |
| --- | --- | --- |
| 1 | APT, Homebrew | Native system packages and Linux formulae. |
| 2 | AppImage, Flatpak | Local AppImage management; explicit Flatpak user/system installations. |
| 3 | DNF, Pacman, Zypper, Snap | Native distro operations and snap lifecycle management. |
| 4 | Cargo, npm, pnpm, Bun | User-installed CLI tools; preserve the selected runtime and installation prefix. |
| 5 | pip, pipx, uv, Composer, RubyGems | Explicit environments and isolated user-tool installations. |

For AppImages, retain the original file, manage an owned copy and desktop entry, and remove only PkgDeck-owned files. Do not execute an imported file merely to inspect its metadata.

Track capabilities and tested manager versions individually. Defer project dependencies, runtime/toolchain management, repository editing, and third-party plugin loading.

### 5. Complete the GUI

- Implement Discover, Search, Package Details, Installed, Updates, Sources, Settings, and Help/About, plus transaction review, progress, and failure states.
- Use Kirigami controls, system theme colors, and virtualized lists or [Qt Quick TableView][qt-tableview]. Support dark mode, keyboard navigation, and accessible labels.
- Show actual metadata, explicit sources/scopes, and only supported actions. Include empty, loading, offline, missing-backend, and authentication-denied states.
- Persist source preferences, environment selections, and settings. Add an operation history with per-backend results; do not invent ratings, download counts, or package equivalence from the mockups.

### 6. Build, package, and release in CI

- Package PkgDeck as Flatpak using the [KDE runtime and Rust packaging workflow][rust-flatpak], as an AppImage with the required Qt/Kirigami libraries and plugins, and as a Snap. Build and package `pkgdeck` and `pkd` together from the same codebase.
- Distribute Flatpak builds through [astrovm/flatpak][flatpak-repository], not Flathub.
- Launch the GUI through `pkgdeck` or its desktop entry and keep `pkd` CLI-only. Document shell access to both bundled entry points in each distribution format and any required host helper. Do not create a separate CLI release or silently install privileged components.
- Test all three release formats on clean target systems, including host commands, authorization, Wayland/X11, Snap confinement, and AppImage launch with and without FUSE.
- Publish versioned artifacts and checksums through GitHub Actions on release tags only after required tests pass. Publish the exact CI-built artifacts that were tested, never manually built replacements. Document prerequisites, supported manager versions, and remaining limitations.

## CI

All builds must run in GitHub Actions, including the `pkgdeck` GUI, the `pkd` CLI, test fixtures, and the Flatpak, AppImage, and Snap packages. Releases must not require local builds or manually prepared artifacts.

- Keep build scripts, packaging manifests, and dependency versions in version control. Each packaging job must build both entry points from the same source commit.
- Run checks, tests, and all three package builds on pull requests without publishing. Retain build artifacts and test logs for review.
- Run the full integration and packaged-application test matrix before release. Keep publishing credentials out of pull-request jobs.
- Publish passing tagged builds through release workflows. Keep Flatpak distribution in [astrovm/flatpak][flatpak-repository], not Flathub; its build and publishing steps must also run in GitHub Actions.

## Testing

Use real package-manager commands, synthetic versioned test packages, and isolated environments. Keep controlled integration tests separate from network-dependent public-store smoke tests.

| Area | Test environment |
| --- | --- |
| APT, DNF, Pacman, Zypper | Separate distro containers with pinned images and controlled repositories; VMs for privileged desktop workflows. |
| Homebrew | A non-root Linux user with a dedicated test tap and isolated prefix. |
| Flatpak | A disposable VM with a local test repository, session D-Bus, and user/system installation tests. |
| Snap | An Ubuntu VM with `snapd`; local snap lifecycle tests and separate Store refresh tests. |
| AppImage | CI-built Type 2 fixtures; [extraction-based launch][appimage-fuse] in containers and normal FUSE launch in a VM. |
| Development managers | Disposable users, homes, prefixes, and virtual environments with controlled package fixtures and registries where needed. |
| GUI, CLI, and packaged app | [Qt Quick Test][qt-tests] for UI behavior; CLI tests without a display server; end-to-end tests of both entry points in each distribution format. |

For each advertised capability, test success, missing packages, malformed output, permission denial, network failure, lock contention, and recovery where applicable. Verify state using the underlying manager, not just PkgDeck's output.

Run fast checks and affected backend tests on pull requests; run the complete container/VM and packaging matrix before release. Mark an operation supported only after its real execution path has passed testing.

## Distribution

The `pkgdeck` GUI and `pkd` CLI will be built and packaged together from the same codebase in GitHub Actions. All distribution artifacts will be produced by CI.

- **Flatpak:** distributed through [astrovm/flatpak][flatpak-repository], not Flathub.
- **AppImage**
- **Snap**

[rust-kirigami]: https://develop.kde.org/docs/getting-started/rust/
[flatpak-spawn]: https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn
[qt-tableview]: https://doc.qt.io/qt-6/qml-qtquick-tableview.html
[rust-flatpak]: https://develop.kde.org/docs/getting-started/rust/rust-flatpak/
[appimage-fuse]: https://docs.appimage.org/user-guide/troubleshooting/fuse.html#extract-and-run-type-2-appimages
[qt-tests]: https://doc.qt.io/qt-6/qtquicktest-index.html
[flatpak-repository]: https://github.com/astrovm/flatpak
