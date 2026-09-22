# Local verification and development tools

Use rootless Podman for local verification: it supplies the toolchain and keeps
GUI tests away from your desktop. Local work and GitHub CI use the same verifier:

```sh
scripts/verify.sh fast --engine podman   # Format, script behavior, Qt-free lint/tests/build
scripts/verify.sh full --only tests --engine podman  # Workspace tests
scripts/verify.sh full --engine podman  # Lint, tests, >=95% coverage, release builds
scripts/verify.sh vm    # Real package/authorization tests in a disposable VM
```

Run these from any directory. Each verification run writes stage logs under
`build/verification/<UTC timestamp>-<mode>-<unique suffix>/`; `build/verification/latest`
points to the latest run. Output streams live to the terminal. A failed stage
stops subsequent stages and preserves its exit status. Stages time out after
20 minutes and terminate their process group; `PKGDECK_STAGE_TIMEOUT` overrides
that limit (GNU timeout syntax). The VM launcher has its own cleanup and deadline.
VM details also remain in `build/host-vm/`. No verification mode publishes releases.

## Running tests

For focused reruns, use the same image and persistent caches:

```sh
scripts/container.sh development --exec cargo test --locked -p pkgdeck --test entrypoint quick_controls
scripts/container.sh development --shell
```

Both commands select the image's Qt/Kirigami environment automatically. The QML
suite uses offscreen rendering; real-window tests use private Xvfb and xdotool.
Neither needs host display sockets.

Native verification remains available by omitting `--engine podman`, after
installing the prerequisites below and running `scripts/setup-dev.sh`.
For direct GUI Cargo tests, first `source scripts/dev-env.sh` to select system
Qt 6.10.2 and Kirigami. Qt-free Cargo tests do not need that environment.

For macOS packaging and the Linux CLI-only Homebrew build, see
[distribution](distribution.md). The native full verifier below targets Linux.

## SDK and prerequisites

Ubuntu 26.04 supplies Qt 6.10.2, Kirigami/ECM 6.24.0, CMake 4.2.3, and Ninja
1.13.2. `setup-dev.sh` installs only cargo-llvm-cov. Qt, KDE, CMake, and Ninja
come from the Ubuntu package archive, so there is no local SDK cache to prepare.

Rust uses the repository's `rust-toolchain.toml`; Cargo verifies registry package checksums.

Prerequisites are rustup/Cargo, a C++ compiler, `libapt-pkg-dev`, jq,
Ninja, pkg-config, LLD, and the Qt/KDE development/runtime
system libraries listed in `.github/actions/setup-desktop/action.yml`. The bootstrap reports
missing command prerequisites before downloading. It does not install host OS
packages or grant authorization.

To use these tools directly:

```sh
source scripts/dev-env.sh
cargo run --locked -p pkgdeck
```

`dev-env.sh` configures PATH, CMake, shared-library, and QML paths. It also finds
LLD in standard distro LLVM directories when it is installed outside PATH. To use
an existing compatible SDK, set `QT_ROOT_DIR` and `PKGDECK_SDK_PREFIX` before
sourcing it. `verify.sh full` checks the Qt version and Kirigami configuration and
reports how to prepare missing tools. Fast checks require no Qt SDK.

Coverage output is `coverage/lcov.info`; `CARGO_TARGET_DIR` can select a separate
compiler cache. The Qt-free check examines both Cargo dependencies and the linked
terminal executable. Script behavior tests use synthetic commands and temporary
log directories; package mutations occur only in isolated lifecycle environments.

Infrastructure changes should be tested through the shared scripts locally,
including a fresh setup and a repeated cached setup when changing bootstrap logic.
CI runs the same checks on x86_64 and aarch64; local x86_64 success does not claim
ARM validation. Run `pinact run --verify` when changing GitHub workflows.

## CI structure and naming

The workflow is named `CI`. Checks use `Category / Scope (architecture)`:

| Check | Responsibility |
| --- | --- |
| `Test / Terminal (x86_64, aarch64)` | Fast checks and Qt-free CLI builds, one job per architecture |
| `Lint / Workspace (x86_64, aarch64)` | Workspace Clippy, one job per architecture |
| `Coverage / Workspace (x86_64)` | Workspace tests with the 95% coverage gate |
| `Test / Workspace (aarch64)` | Native workspace tests without instrumentation |
| `Test / Podman (x86_64, aarch64)` | Container workspace tests and CLI APT/Homebrew lifecycles, one job per architecture |
| `Test / VM (x86_64)` | VM lifecycles and authorization checks |
| `Test / Backend / <backend> (x86_64)` | Real native/development-manager lifecycle tests |
| `Package / Linux (x86_64, aarch64)` | Release build, package formats, and packaged GUI lifecycles, one job per architecture |

Job IDs and log stages use lowercase kebab-case (`test-workspace`,
`lint-terminal`, `build-release`, `check-coverage`). Step names start with an
action: `Install`, `Build`, `Run`, or `Upload`. Artifacts use
`<kind>-<scope>-<architecture>`, for example `logs-podman-aarch64`.
Rust tests retain descriptive snake_case names; Qt Quick uses its standard
`tst_*.qml` files and `test_*` functions.

Desktop jobs share `.github/actions/setup-desktop`: packaging tools are installed
only for packaging, and cargo-llvm-cov only for coverage. Packaging owns release
builds; Podman exercises the container test environment without repeating native
lint, coverage, and release stages.

Native Rust caches are separated by architecture and purpose (terminal, lint,
tests, coverage, release). Keys include dependency/toolchain inputs and the source
revision, with fallback to the latest compatible build. Backend jobs restore the
terminal cache without competing to save it. Container caches are separate and
include the development-image inputs. Incremental compiler directories are
excluded from uploads to reduce cache size. A new cache namespace starts cold;
subsequent successful runs populate it.

## Rootless Podman

For workspace verification, only Bash/coreutils and a working rootless Podman
installation are needed on the host:

```sh
scripts/verify.sh full --engine podman
scripts/verify.sh fast --engine podman
scripts/verify.sh containers --engine podman
```

`containers` builds the CLI in the development image, then runs real APT and
Homebrew lifecycles in a separate disposable container. Without `--engine podman`,
it builds the CLI using host Cargo before running the same isolated tests.
The development image includes the pinned Rust toolchain and the Ubuntu Qt/Kirigami
packages, and uses the exact `setup-dev.sh`/`verify.sh` recipes from native
development and CI.
Both base images are pinned to multi-architecture manifest digests. It supports
x86_64 and aarch64; lifecycle tests use the host architecture.

Images are tagged by their recipe/tool inputs and architecture. Cargo downloads
and build outputs persist in separate directories below
`$XDG_CACHE_HOME/pkgdeck/containers` (or `$HOME/.cache/pkgdeck/containers`), scoped
by checkout path and development-image key. `PKGDECK_CONTAINER_CACHE` overrides
that location. `--userns=keep-id` gives build outputs the invoking user's ownership.
Host `target/` is hidden by the container's compiler-cache mount; the two builds
do not mix artifacts. The checkout is writable for build-generated configuration
and logs. Host HOME, Cargo credentials, package databases, and daemon sockets are
not mounted. GUI verification uses offscreen rendering or private Xvfb and needs
no desktop socket. Warm runs reuse the container's downloads and compiled output;
the first run also builds the image and dependencies.

The lifecycle container mounts only the checkout and selected binaries read-only.
Root inside its user namespace can prepare synthetic users and package fixtures,
but it has no host-root privileges. Homebrew itself runs as an unprivileged guest
user at `/home/linuxbrew/.linuxbrew`. Each run starts a new writable container
layer, which is removed on exit. The image contains dependencies only, with no
installed synthetic packages or test authorization grants.

Ubuntu and Rust base contents, Qt/KDE versions, and Homebrew 6.0.22 sources are
pinned; the Homebrew tarball has a committed SHA-256. OS dependency installation
still uses the distribution's current signed repositories. These builds are
therefore reproducible from a retained image, not bit-for-bit reproducible rebuilds
across changing OS repositories. Resolved OS packages are recorded at
`/opt/pkgdeck-os-packages.txt` in each prepared image. Delete a particular PkgDeck
image tag to rebuild it; the scripts never prune unrelated images or caches.

## Prepared QEMU guests

```sh
scripts/verify.sh vm  # First run prepares dependencies, then tests a fresh overlay
scripts/verify.sh vm  # Reuses the prepared base; creates another fresh test overlay
```

The preparation stage installs system dependencies and the checksum-verified
Homebrew tool once. It does not install test packages or create the privileged test
user. Its cache key includes the pinned Ubuntu cloud image checksum and
`scripts/vm/prepare.sh`; changing lifecycle assertions does not invalidate setup.
Prepared images have a SHA-256 sidecar checked before reuse. A failed preparation
never becomes a valid cache entry. A cache lock serializes runs using the same base.

Set `PKGDECK_VM_CACHE` to choose the cache directory (default `build/host-vm`). Keep
it at a stable absolute path because qcow2 backing paths are absolute. Logs stream
live and remain in `build/host-vm/logs/<run>-prepare|lifecycle/`. Each boot has a
15-minute deadline; the shared verifier also bounds the overall VM stage. Failed
runs retain logs and remove disposable overlays. SIGTERM and Ctrl-C unwind QEMU
cleanup. `CARGO_TARGET_DIR` selects binaries outside the default host target path;
they are mounted separately and read-only.

QEMU retains the real sudo/polkit, APT lock, and full-system checks. Podman covers
faster backend lifecycles; it does not claim desktop authorization, installed
Flatpak/Snap bridges, FUSE, or a real display session. The current QEMU fixture is
x86_64 only. No preparation or test script is intended to run directly on the host.

## Native tooling

`cargo xtask` runs the Rust acceptance drivers (`gui`, `gui-failure`,
`gui-lifecycle`, `gui-write`, `apt-lock-probe`, and `qml`). Build it once with
`cargo build -p pkgdeck-tools`; `scripts/xtask.sh` invokes that binary for package
checks. GUI drivers use private Xvfb servers and temporary settings directories.
The 95% coverage gate continues to measure application Rust code and its build
script; test drivers are excluded. Their behavior is exercised by the frontend
acceptance tests and disposable guest lifecycles.

`scripts/build-apt.sh` builds the separate GPL-2.0-or-later APT reader using the
host's `libapt-pkg-dev` package. Its sibling placement keeps APT out of frontend
link dependencies. `scripts/bundle.sh` includes its library closure and license
notices, so users do not need to install development headers or language bindings.
The installed host still supplies the native package manager and authorization
agents used for transactions.

`scripts/check-no-python.sh` rejects project-owned Python scripts and interpreter
invocations. This does not constrain dependencies of the host OS, cloud-init,
Qt's upstream SDK, or independently installed package managers.
