# PkgDeck

A unified package manager interface for Linux, built with Rust and Qt/Kirigami, with the `pkgdeck` GUI and the `pkd` CLI built together from one codebase.

## Preview

Current design mockups; implementation is in progress.

### GUI — `pkgdeck`

![PkgDeck GUI](docs/screenshots/pkgdeck-gui.png)

### CLI — `pkd`

![pkd CLI](docs/screenshots/pkd-cli.png)

## Planned initial support

- **System:** APT, DNF, Pacman, Zypper.
- **Applications:** Flatpak, Snap, Homebrew (Linux formulae), AppImage.
- **Development:** Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems.

Development support will initially focus on user-installed command-line tools, with pip restricted to explicitly selected virtual environments.

AppImage support will initially cover importing local Type 2 AppImages, desktop integration, launching, and removal. Automatic AppImage updates are outside the initial scope.

## Implementation plan

The following phases describe planned work, not implemented features.

### 1. Set up the project

- Create a Cargo workspace with a shared `pkgdeck-core` library and thin `pkgdeck` GUI and `pkd` CLI entry points. Build and package both together. Keep all package-management logic shared and the core independent of Qt. The `pkd` entry point is CLI-only and must not initialize the GUI.
- Connect Rust to Qt 6/QML and Kirigami through CXX-Qt, following [KDE's Rust integration guide][rust-kirigami]. Use CMake to coordinate application builds and installation.
- Pin compatible Rust, Qt, Kirigami, and CXX-Qt versions and commit the dependency lockfile. Select the initial target architectures and record the supported host environments.
- Build minimal packages in all three formats early, using the workflows described in [CI](#ci). Validate the launchers and host-execution path before expanding backend coverage.

### 2. Build the shared engine and host-execution layer

- Define backend capabilities for detection, search, details, installed packages, update checks, installation, upgrades, and removal. Expose unsupported operations explicitly.
- Identify packages by backend, repository, package ID, architecture, and installation scope. Group equivalent applications only with verified mappings, never by matching names alone.
- Prefer documented APIs or structured output. Isolate and test required text parsers. Detect manager versions and enable only compatible backends for the selected host and scope.
- Execute commands with argument arrays, not shell strings. Validate package identifiers and options, capture progress and exit codes, and keep operations off the GUI thread.
- Coordinate writes across GUI and CLI processes per package database, respect native locks, and allow cancellation only at backend-safe points. Report partial failures without promising cross-manager rollback.

#### Host access and authorization

- Resolve package managers, package databases, user homes, and development-tool environments on the **host**, not inside the packaging runtime. Use explicit host executable paths and selected prefixes or virtual environments rather than sourcing shell startup files.
- For Flatpak, use [flatpak-spawn --host][flatpak-spawn] with the required `org.freedesktop.Flatpak` D-Bus permission. Document that this permits unsandboxed host commands; it does not grant root privileges or automatically provide a correct host environment.
- For AppImage and the planned classic Snap, execute host commands directly with a controlled host environment. Do not leak bundled library or plugin paths into host package-manager processes.
- Keep both entry points unprivileged. Use existing authorized host services where appropriate, or narrowly scoped polkit-authorized host operations with validated requests and trusted executable paths. Never elevate Homebrew or user-scoped development tools.
- Before enabling privileged writes, document the authorization path for each backend. Any required host helper must have an explicit, versioned installation, update, compatibility-check, and removal procedure; build it through the same CI workflow. Do not silently install privileged components or accept arbitrary root shell commands.
- Support graphical authorization for the GUI and terminal authorization for the CLI where available. Fail clearly when an agent, session bus, permission, or required helper is missing. Non-interactive commands must not hang or open the GUI.

**Acceptance criterion:** from each CI-built distribution format, demonstrate host manager detection, an unprivileged Homebrew install/remove operation, and an authorized system-package install/remove operation through both entry points. Verify changes with the host manager. Keep unvalidated operations disabled.

### 3. Deliver the first complete workflow

- Implement APT and Homebrew first, with controlled test packages covering installation, inspection, upgrades, and removal.
- Add `pkd search`, `info`, `list`, `sources`, `install`, `remove`, `update`, and `upgrade`. Support `--from` where applicable, machine-readable `--json` output, and consistent exit codes. `update` refreshes metadata/checks availability; `upgrade` applies changes.
- Require an explicit source when a package is ambiguous. Preview changes where supported and confirm destructive operations. Preserve each manager's transaction and upgrade rules rather than forcing identical behavior.
- Connect GUI search, package details, and installed-package views to the same engine. Complete one real package lifecycle through both interfaces before adding more backends.

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

Track capabilities and tested manager versions individually, including differences between distribution formats. Defer project dependencies, runtime/toolchain management, repository editing, and third-party plugin loading.

### 5. Complete the GUI

- Implement Discover, Search, Package Details, Installed, Updates, Sources, Settings, and Help/About, plus transaction review, progress, and failure states.
- Use Kirigami controls, system theme colors, and virtualized lists or [Qt Quick TableView][qt-tableview]. Support dark mode, keyboard navigation, and accessible labels.
- Show actual metadata, explicit sources/scopes, and only supported actions. Include empty, loading, offline, missing-backend, and authentication-denied states.
- Persist source preferences, environment selections, and settings. Add an operation history with per-backend results; do not invent ratings, download counts, or package equivalence.

### 6. Package both entry points and connect publishing

Package `pkgdeck` and `pkd` together in every format. Do not create a separate CLI release.

#### Launchers

The planned application ID is `io.github.astrovm.PkgDeck`. Implement and test these launch paths; these are packaging targets, not instructions for an existing release.

| Format | GUI | CLI |
| --- | --- | --- |
| Flatpak | `flatpak run io.github.astrovm.PkgDeck` | `flatpak run --command=pkd io.github.astrovm.PkgDeck <arguments>` |
| AppImage | `./PkgDeck.AppImage` | `./PkgDeck.AppImage --cli <arguments>` |
| Snap | `pkgdeck` | `pkgdeck.pkd <arguments>` |

- **Flatpak:** use the [KDE runtime and Rust packaging workflow][rust-flatpak], set `pkgdeck` as the default command, and bundle `pkd` as the alternative [run command][flatpak-run]. Provide optional user-installed wrappers for the short commands. Test terminal I/O, host paths, and session-bus requirements without a display server.
- **AppImage:** bundle the required Qt/Kirigami libraries and plugins. Implement an [AppRun launcher][appdir] that starts `pkgdeck` by default and dispatches `--cli` to the bundled `pkd`, removing only the dispatch flag. This is launcher routing, not a GUI mode in `pkd`. Provide optional wrappers pointing to the installed AppImage path.
- **Snap:** declare both application entry points. Snap's [command namespacing and aliases][snap-aliases] make the CLI available as `pkgdeck.pkd`; document the optional manual alias `sudo snap alias pkgdeck.pkd pkd`. Do not assume an automatic alias will be approved.

Wrappers must forward arguments, standard input/output, signals, and exit status correctly, and must not overwrite existing commands without consent.

#### Snap distribution

Build a classic-confinement `.snap` for explicit sideloading from GitHub Releases, and validate host execution and authorization before marking its operations supported. Document its broad host access and installation requirements. Do not ship a release that depends on `devmode`.

Snap Store publication is not an initial release requirement. The [classic-confinement review policy][snap-classic-review] lists management snaps and third-party installer snaps as unsupported categories, so Store approval must not be assumed. [Manual installation outside the Store][snap-classic] is a separate distribution route. Any later Store publication requires a permitted confinement model and the relevant approvals.

This distribution constraint does not remove the Snap package-management backend from the planned scope.

#### Flatpak publishing

Use the existing [astrovm/flatpak publishing workflow][flatpak-publishing], not a Flathub application submission or a second application build.

- Register `astrovm/PkgDeck` in the publisher's `apps.json` with application ID `io.github.astrovm.PkgDeck`, bundle prefix `PkgDeck`, the matching Flatpak branch, tested architectures, and the runtime repository. Keep these values aligned with the application manifest and release jobs.
- Build and test one bundle per configured architecture, named `PkgDeck-<architecture>.flatpak`. Publish the exact tested bundles and their digests in an immutable GitHub release.
- After publication, send a `publish-app` repository dispatch to `astrovm/flatpak`, with `client_payload.repository` set to `astrovm/PkgDeck` and `client_payload.tag` set to the released tag.
- Let the publisher validate, import, and sign the bundles, regenerate repository metadata and installers, and verify the result with a fresh Flatpak client. It must not rebuild PkgDeck.
- Verify installation and updates from the resulting repository. Keep signing keys in the publisher and dispatch credentials in protected release jobs, following its documented secret setup.

PkgDeck's application will be distributed through the astrovm repository. A separately configured runtime source, including Flathub where needed for the KDE runtime, does not change the application's distribution channel.

## CI

All builds must run in GitHub Actions: the `pkgdeck` GUI, the `pkd` CLI, any required helper, test fixtures, and the Flatpak, AppImage, and Snap packages. Releases must not require local builds or manually prepared artifacts.

- **Build configuration:** keep scripts, manifests, toolchain versions, dependency locks, and test-environment definitions in version control. Pin CI actions and container images. Build both entry points from the same commit in each packaging job.
- **Pull requests:** run formatting, linting, unit tests, affected backend tests, and builds of all three package formats. Retain artifacts and logs. Do not publish or expose publishing/signing credentials to pull-request code.
- **Release tags:** build the tagged commit and run the full backend and packaged-application matrix against the resulting artifacts. Publish only the exact passing artifacts and checksums; do not rebuild after testing.
- **Publishing:** attach artifacts before finalizing an immutable GitHub release, then dispatch Flatpak publication to `astrovm/flatpak`. Its import, signing, publishing, and verification steps must also run in GitHub Actions. Keep AppImage and sideloadable Snap artifacts in the release.
- **Coverage:** provision disposable containers and VMs through CI. Record supported architectures, host distributions, manager versions, and format-specific limitations. Do not advertise an untested target as supported.

## Testing

Use real package-manager commands, synthetic versioned test packages, and isolated environments. Keep controlled integration tests separate from network-dependent public-store smoke tests.

| Area | Test environment |
| --- | --- |
| APT, DNF, Pacman, Zypper | Separate distro containers with pinned images and controlled repositories; VMs for privileged desktop workflows. |
| Homebrew | A disposable environment with a non-root user, a controlled tap, and the [standard Linux prefix][brew-linux] `/home/linuxbrew/.linuxbrew`. Isolate the environment rather than moving the prefix. |
| Flatpak backend | A disposable VM with a local test repository, session D-Bus, and user/system installation tests. |
| Snap backend | An Ubuntu VM with `snapd`; local snap lifecycle tests and separate Store refresh tests. |
| AppImage backend | CI-built Type 2 fixtures; [extraction-based launch][appimage-fuse] in containers and normal FUSE launch in a VM. Test metadata inspection without executing the imported file. |
| Development managers | Disposable users, homes, prefixes, and virtual environments with controlled package fixtures and registries where needed. |
| GUI and CLI | [Qt Quick Test][qt-tests] for UI behavior; CLI tests without a display server, including terminal and non-interactive authorization paths. |
| Packaged application | End-to-end tests of both bundled entry points in each format, including wrappers/aliases, host detection, authorization, Wayland/X11, AppImage FUSE/extraction, and classic Snap sideloading. |
| Flatpak publishing | Verify release bundle names, digests, refs, architectures, publisher registration, and installation/update through the signed astrovm repository. |

For each advertised capability, test success, missing packages, malformed output, permission denial, missing authorization agents, network failure, lock contention, and recovery where applicable. Verify state with the host's underlying manager, not just PkgDeck's output. Check that GUI and CLI see the same selected installation scope and coordinate concurrent writes.

Mark an operation supported only after its real execution path has passed testing in the advertised environments.

## Distribution

Both entry points are included in each CI-built package:

- **Flatpak:** [astrovm/flatpak][flatpak-repository], served from [flatpak.4st.li][flatpak-site], not Flathub.
- **AppImage:** GitHub Releases.
- **Snap:** a sideloadable classic `.snap` in GitHub Releases; Store publication is not assumed.

[rust-kirigami]: https://develop.kde.org/docs/getting-started/rust/
[flatpak-spawn]: https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn
[flatpak-run]: https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-run
[qt-tableview]: https://doc.qt.io/qt-6/qml-qtquick-tableview.html
[rust-flatpak]: https://develop.kde.org/docs/getting-started/rust/rust-flatpak/
[appdir]: https://docs.appimage.org/reference/appdir.html
[appimage-fuse]: https://docs.appimage.org/user-guide/troubleshooting/fuse.html#extract-and-run-type-2-appimages
[snap-aliases]: https://snapcraft.io/docs/how-to-guides/manage-snaps/apps-and-aliases/
[snap-classic-review]: https://snapcraft.io/docs/reference/administration/reviewing-classic-confinement-snaps/
[snap-classic]: https://snapcraft.io/docs/explanation/security/classic-confinement/
[brew-linux]: https://docs.brew.sh/Homebrew-on-Linux
[qt-tests]: https://doc.qt.io/qt-6/qtquicktest-index.html
[flatpak-repository]: https://github.com/astrovm/flatpak
[flatpak-publishing]: https://github.com/astrovm/flatpak#publishing
[flatpak-site]: https://flatpak.4st.li/
