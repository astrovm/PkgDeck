# Local verification and development tools

Local work and GitHub CI use the same entry points:

```sh
scripts/verify.sh fast   # Format, script behavior, Qt-free lint/tests/build
scripts/setup-dev.sh    # Once per SDK/tool version; safe to repeat
scripts/verify.sh full  # Workspace lint, tests, >=95% coverage, release builds
scripts/verify.sh vm    # Real package/authorization tests in a disposable VM
```

Run these from any directory. Each verification run writes stage logs under
`build/verification/<UTC timestamp>-<mode>-<unique suffix>/`; `build/verification/latest`
points to the latest run. Output streams live to the terminal. A failed stage
stops subsequent stages and preserves its exit status. Stages time out after
20 minutes and terminate their process group; `PKGDECK_STAGE_TIMEOUT` overrides
that limit (GNU timeout syntax). The VM launcher has its own cleanup and deadline.
VM details also remain in `build/host-vm/`. No verification mode publishes releases.

## SDK and prerequisites

`setup-dev.sh` uses a persistent per-architecture cache at
`${XDG_CACHE_HOME:-$HOME/.cache}/pkgdeck/<architecture>`. Set `PKGDECK_CACHE_DIR`
to choose another location, for example `build/dev-cache` in CI. Keep this path
stable: the Python environment and some generated SDK configuration use absolute
paths. Concurrent setup processes share a cache lock.

The setup installs Qt 6.11.2, Kirigami/ECM 6.30.0, aqtinstall 3.3.0, CMake 4.4.3,
and cargo-llvm-cov 0.9.1 without replacing system packages or global Cargo tools.
Rust uses the repository's `rust-toolchain.toml`. KDE source archives are checked
against committed SHA-256 hashes in `scripts/sdk.sha256`; Qt downloads use aqt's
upstream archive verification, and Cargo verifies registry package checksums.
Python dependencies use pinned top-level versions and pip's normal transport
verification; they are not a fully hashed transitive Python lockfile.

Prerequisites are Python **3.14** with venv support, rustup/Cargo, a C++ compiler,
Ninja, pkg-config, LLD, curl, tar, flock, and the Qt development/runtime system
libraries listed in `.github/workflows/ci.yml`. Python 3.14 is required because
older Python versions can misidentify the QtTools archives. The bootstrap reports
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
The development image includes the pinned Rust and Qt/Kirigami toolchains and
uses the exact `setup-dev.sh`/`verify.sh` recipes from native development and CI.
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
not mounted. GUI verification uses offscreen rendering and needs no desktop socket.

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
`scripts/vm/prepare.py`; changing lifecycle assertions does not invalidate setup.
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
