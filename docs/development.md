# Development

## Quick start

The easiest way to build and test is with rootless Podman. It provides the
whole toolchain and runs GUI tests off-screen, away from your desktop. CI uses
the same scripts.

```sh
scripts/verify.sh fast --engine podman                # formatting, script tests, Qt-free lint/tests/build
scripts/verify.sh full --only tests --engine podman   # workspace tests only
scripts/verify.sh full --engine podman                # lint, tests, ≥96% coverage, release builds
scripts/verify.sh containers --engine podman          # real APT and Homebrew install/remove tests
```

You can run these from any directory. Nothing is ever published.

- Logs go to `build/verification/<UTC timestamp>-<mode>-<suffix>/`, and
  `build/verification/latest` points to the newest run. Output also streams to
  the terminal.
- The first failed stage stops the run and keeps its exit code.
- Each stage times out after 20 minutes. Set `PKGDECK_STAGE_TIMEOUT` to change
  it (GNU `timeout` format).

## Running single tests

Use the same container image and caches:

```sh
scripts/container.sh development --exec cargo test --locked -p pkgdeck --test entrypoint quick_controls
scripts/container.sh development --shell
```

Both set up Qt and Kirigami for you. QML tests render off-screen, and
real-window tests use a private Xvfb display with xdotool. Neither touches
your display.

To test without Podman, install the prerequisites below, run
`scripts/setup-dev.sh`, and drop `--engine podman`. Before running GUI tests
with Cargo directly, run `source scripts/dev-env.sh`. Qt-free tests don't
need it.

For the macOS app bundle and the Linux CLI-only Homebrew package, see
[distribution](distribution.md). The native `full` check targets Linux.

## SDK and prerequisites

Ubuntu 26.04 provides Qt 6.10.2, Kirigami/ECM 6.24.0, CMake 4.2.3, and Ninja
1.13.2 from its normal package archive. There's no separate SDK to download.
`setup-dev.sh` only installs `cargo-llvm-cov`. Rust comes from
`rust-toolchain.toml`.

You need:

- rustup and Cargo
- a C++ compiler
- `libapt-pkg-dev`, jq, Ninja, pkg-config, and LLD
- the Qt/KDE libraries listed in `.github/actions/setup-desktop/action.yml`

`setup-dev.sh` tells you which commands are missing. It never installs system
packages or changes permissions.

To build and run the app:

```sh
source scripts/dev-env.sh
cargo run --locked -p pkgdeck
```

`dev-env.sh` sets up `PATH`, CMake, library, and QML paths. It also finds LLD
in standard LLVM folders when it isn't on `PATH`. To use your own compatible
SDK, set `QT_ROOT_DIR` and `PKGDECK_SDK_PREFIX` before sourcing it.
`verify.sh full` checks your Qt and Kirigami setup and tells you what's missing.
`fast` doesn't need Qt.

### Notes

- Coverage is written to `coverage/lcov.info`. Set `CARGO_TARGET_DIR` to use a
  separate build cache.
- The Qt-free check looks at both Cargo dependencies and the final `pkd`
  binary.
- Script tests use fake commands and temporary log folders. Real package
  changes only happen inside disposable containers.
- When changing setup scripts, test a fresh setup and a second, cached setup.
- CI runs on x86_64 and aarch64. Passing on x86_64 locally doesn't prove ARM
  works.
- Run `pinact run --verify` after changing GitHub workflows.

## CI

The workflow is called `CI`. Check names follow `Category / Scope (architecture)`:

| Check | What it does |
| --- | --- |
| `Test / Linux (x86_64, aarch64)` | Everything on Linux once per architecture: format and infrastructure checks, the APT helper, clippy (x86_64), all workspace tests (measured for coverage on x86_64), and real sudo/polkit and APT lock tests |
| `Test / macOS native (x86_64, aarch64)` | All core/CLI tests (measured for coverage on Apple Silicon), real app inventory, Homebrew cask ownership and adoption, including recovery from a failed adoption |
| `Coverage / Combined` | The 96% gate: merges the Linux and macOS reports; a line counts once and is covered when a test ran it on any platform |
| `Test / Podman (x86_64, aarch64)` | CLI build in the development container and CLI APT/Homebrew install/remove tests |
| `Test / Backends / native (linux-x86_64, linux-aarch64)` | Real DNF, Zypper, apk, XBPS, Snap, Toolbx and Distrobox lifecycle tests, plus Pacman and the AUR on x86_64 |
| `Test / Backends / dev (<platform>)` | Real lifecycle tests for every development manager on Linux and macOS, x86_64 and aarch64, and MacPorts on macOS; the job summary lists each backend's result |
| `Package / AppImage + Snap (x86_64, aarch64)` | Release build, packages, and GUI tests on the packaged app |
| `Package / Flatpak (x86_64, aarch64)` | Flatpak build, installed GUI, and host bridge tests |
| `Package / macOS app (x86_64, aarch64)` | Self-contained `PkgDeck.app` build, bundle checks, GUI/controller, QML, startup and media tests, then installs that app through a Homebrew cask and checks it starts |
| `Package / Linux CLI (x86_64, aarch64)` | Static `pkd` and host runner archive, tested on Alpine |
| `Test / Homebrew (Linux)` | Renders the tap from the Linux packages and installs and tests the formula; published recipes are checked on macOS too |
| `Prune superseded caches` | On main only: keeps the newest copy of each build cache so the 10 GB limit never evicts the ones pull requests restore |

The macOS native job uses real `plutil` with XML and binary bundles, system and
user application directories, a locally built cask installed by Homebrew, and
actual permission-denied folders. It checks exact-copy ownership, duplicate
copies, App Store receipts, malformed metadata, nested/helper apps, symlinks,
partial results, details, search, and rejection of writes. Fixtures have an
executable launch marker that must remain absent. JSON reports and lifecycle
logs are uploaded as `macos-native-<architecture>` artifacts. The script is
restricted to disposable GitHub-hosted runners because it creates bundles in
`/Applications` and installs a temporary cask. It does not validate signed vendor
apps, adoption, or migration; those writes remain unsupported.

The [platform test audit](platform-test-audit.md) records which checks use real
managers, which use fixtures, and which require hardware or a native desktop.

Naming rules:

- Job IDs and log stages use kebab-case: `test-workspace`, `lint-terminal`,
  `build-release`, `check-coverage`.
- Step names start with a verb: `Install`, `Build`, `Run`, or `Upload`.
- Artifacts use `<kind>-<scope>-<architecture>`, like `logs-podman-aarch64`.
- Rust tests use descriptive snake_case names. Qt Quick tests use `tst_*.qml`
  files and `test_*` functions.
- A test name is a short sentence about the behavior it checks, like
  `failed_update_check_keeps_known_updates`. Don't name tests after a list of
  topics, a development phase, or the toolkit that runs them.

Desktop jobs share `.github/actions/setup-desktop`. Packaging tools are only
installed for packaging jobs, and `cargo-llvm-cov` only for coverage. Release
builds happen in packaging jobs. The Podman job doesn't repeat lint, coverage,
workspace tests, or release builds.

Rust caches are split by architecture and purpose (terminal, lint, tests,
coverage, release, macOS native, Homebrew GUI tests). Cache keys include
dependencies, toolchain, and source revision, and fall back to the newest cache
for the same dependencies, then to any cache of that purpose. Only pushes to
main save caches; they rerun the cache-owning jobs so every pull request starts
from main's build outputs. Pull requests restore without saving, because GitHub
never shares one pull request's caches with another. Backend jobs reuse the
terminal cache without saving it. Container caches are separate. Incremental
build folders aren't uploaded.

## Podman details

On the host you only need Bash, coreutils, and rootless Podman:

```sh
scripts/verify.sh full --engine podman
scripts/verify.sh fast --engine podman
scripts/verify.sh containers --engine podman
```

`containers` builds the CLI in the development image, then runs real APT and
Homebrew install/remove tests in a separate throwaway container. Without
`--engine podman`, it builds the CLI with your local Cargo first.

The development image has the pinned Rust toolchain and Ubuntu's Qt/Kirigami
packages. It runs the same `setup-dev.sh` and `verify.sh` as native builds and
CI. Base images are pinned by digest and support x86_64 and aarch64. Tests use
your machine's architecture.

### Caches

Images are tagged by their inputs and architecture. Cargo
downloads and build output are kept in `$XDG_CACHE_HOME/pkgdeck/containers`
(or `~/.cache/pkgdeck/containers`), separated by checkout path and image. Set
`PKGDECK_CONTAINER_CACHE` to move them. Later runs reuse them. The first run
also builds the image and dependencies.

### Isolation

- Files are created with your user ID (`--userns=keep-id`).
- Your local `target/` is hidden inside the container, so the two builds never
  mix.
- The checkout is writable so builds can write config and logs.
- Your home folder, Cargo credentials, package databases, and daemon sockets
  are never mounted.
- GUI tests use off-screen rendering or a private Xvfb display.

The install/remove test container only mounts the checkout and the built
binaries, read-only. Root inside the container can create test users and
packages but has no root access on your machine. Homebrew runs as a normal
user at `/home/linuxbrew/.linuxbrew`. Each run starts fresh and is deleted
afterward.

### Reproducibility

Ubuntu and Rust base images, Qt/KDE versions, and
Homebrew 7.0.6 are pinned. The Homebrew tarball has a committed SHA-256. OS
packages still come from the distro's current signed repositories, so a
rebuild can differ from a saved image. The exact OS package list is saved to
`/opt/pkgdeck-os-packages.txt` in each image. To rebuild an image, delete its
tag. The scripts never delete unrelated images or caches.

## Authorization tests in CI

CI runs `scripts/tests/host-authorization.sh` as root on a fresh GitHub-hosted
Ubuntu 26.04 x86_64 runner. The script refuses to run anywhere else. It
creates a test package and test users, then checks:

- that package managers are detected
- that unauthorized users are denied
- APT lock handling
- install and remove through both sudo and polkit

It cleans up afterward, and GitHub discards the runner.

APT and Homebrew install/remove tests also run in rootless Podman on both
architectures. These don't cover desktop password prompts, installed
Flatpak/Snap bridges, FUSE, or a real display.

## Tools

- `cargo xtask` runs the Rust acceptance tests: `gui`, `gui-failure`,
  `gui-lifecycle`, `gui-write`, `apt-lock-probe`, and `qml`. Build it once with
  `cargo build -p pkgdeck-tools`; `scripts/xtask.sh` runs it for package
  checks. GUI tests use private Xvfb displays and temporary settings. These
  test tools don't count toward the 96% coverage gate.
- `scripts/build-apt.sh` builds the separate APT reader (GPL-2.0-or-later)
  from `libapt-pkg-dev`. It stays a separate program so APT isn't linked into
  the app. `scripts/bundle.sh` bundles its libraries and license notices, so
  users don't need development headers. Package changes still use the
  system's own package manager and password prompt.
- `scripts/check-no-python.sh` rejects Python scripts and Python calls in
  this project. It doesn't apply to the OS, cloud-init, Qt, or package
  managers.
