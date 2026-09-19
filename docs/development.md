# Local verification and development tools

Local work and GitHub CI use the same entry points:

```sh
scripts/verify.sh fast   # Format, script behavior, Qt-free lint/tests/build
scripts/setup-dev.sh    # Once per SDK/tool version; safe to repeat
scripts/verify.sh full  # Workspace lint, tests, >=95% coverage, release builds
scripts/verify.sh full --only lint,coverage,release,tests  # Full-mode subset for parallel CI jobs
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

Use these entry points; they match CI exactly. Anything else is unsupported.

```sh
scripts/verify.sh fast                      # no Qt needed
source scripts/dev-env.sh                   # once per shell, required below
cargo test --locked -p pkgdeck-core -p pkd  # Rust unit + integration tests
cargo test -p pkgdeck --test entrypoint quick_controls   # QML suite, headless
cargo test -p pkgdeck --test entrypoint     # above plus the real-window lifecycle
scripts/verify.sh full --only tests         # everything above, as CI runs it
```

`source scripts/dev-env.sh` selects the system Qt 6.10.2 toolchain
(`qmltestrunner`, Kirigami imports) and must precede any direct `cargo`
invocation. The QML suite runs `qmltestrunner` offscreen with temporary XDG
directories; it needs no display and never touches your session.
`real_window` additionally drives the built app on a private Xvfb server
via `xdotool` — also fully isolated — and needs `xvfb`, `xdotool`, and
fonts (`xvfb xdotool fonts-dejavu-core`, same list CI installs).

Do not:
- run `qmltestrunner -input ...` by hand: tool and Kirigami resolution
  depend on the `dev-env.sh` environment;
- prepend another Qt SDK's `bin/` to `PATH`: it hijacks `qmltestrunner`
  and breaks imports with misleading "module not installed" errors;
- set `DISPLAY` or drive the app manually: the harness owns private
  displays, and GUI tests never need your session.

If a suite fails, match the symptom:
- `qmltestrunner: No such file` → `dev-env.sh` was not sourced.
- `Type App.Browser unavailable` / `module "org.kde.kirigami" is not
  installed → `QML_IMPORT_PATH` lacks Kirigami; source `dev-env.sh` and
  remove any foreign Qt from `PATH`. The harness now preflights both and
  says so directly.
- `Xvfb`/`xdotool: No such file` → install the GUI test prerequisites.
- `state.json missing ... fixture backend never ran` (`real_window`) →
  the scripted drive stalled before any write; inspect the attached
  `application.log`, and confirm Xvfb/xdotool/software rendering work
  (CI's desktop job is the reference environment).

## SDK and prerequisites

Ubuntu 26.04 supplies Qt 6.10.2, Kirigami/ECM 6.24.0, CMake 4.2.3, and Ninja
1.13.2. `setup-dev.sh` installs only cargo-llvm-cov. Qt, KDE, CMake, and Ninja
come from the Ubuntu package archive, so there is no local SDK cache to prepare.

Rust uses the repository's `rust-toolchain.toml`; Cargo verifies registry package checksums.

Prerequisites are rustup/Cargo, a C++ compiler, `libapt-pkg-dev`, jq,
Ninja, pkg-config, LLD, and the Qt/KDE development/runtime
system libraries listed in `.github/workflows/ci.yml`. The bootstrap reports
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

`cargo xtask` runs the Rust acceptance drivers (`terminal`, `terminal-interactions`,
`gui`, `gui-failure`, `gui-lifecycle`, and `qml`). Build it once with
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
