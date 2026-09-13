# Local verification and development tools

Local work and GitHub CI use the same entry points:

```sh
scripts/verify.sh fast   # Format, script behavior, Qt-free lint/tests/build
scripts/setup-dev.sh    # Once per SDK/tool version; safe to repeat
scripts/verify.sh full  # Workspace lint, tests, >=95% coverage, release builds
scripts/verify.sh vm    # Real package/authorization tests in a disposable VM
```

Run these from any directory. Each verification run writes stage logs under
`build/verification/<UTC timestamp>-<pid>-<mode>/`; `build/verification/latest`
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
