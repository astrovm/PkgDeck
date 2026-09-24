# Host execution and authorization

`pkgdeck-core` provides the shared execution boundary. `pkd doctor` reports its
runtime, executes a bounded unprivileged host architecture probe, and locates
backend executables in the invoking user's PATH. Each adapter advertises its
supported operations; detection alone does not imply every operation is supported.

## Format capabilities

| Format | Host reads/detection | Host writes | Validation |
| --- | --- | --- | --- |
| Native | Enabled | Enabled through the core and CLI | Synthetic process tests; real sudo/polkit and APT on a disposable GitHub-hosted Ubuntu x86_64 runner |
| AppImage | Enabled, outside the bundle | Enabled through the same native boundary | Extract-and-run AppImage doctor probe, GUI and terminal smoke tests; environment-isolation tests |
| Flatpak | Host managers through `flatpak-spawn --host` | Same typed operations and authorization as native | Installed host APT query, synthetic Homebrew lifecycle, and Flatpak lifecycle |
| Snap (classic) | Enabled, outside `$SNAP` | Enabled through the same native boundary | Runtime isolation tests and installed package smoke tests in CI |

The format gate is checked for reads, executable discovery, and authorized writes.
Snap and Flatpak markers take precedence over AppImage markers. The Flatpak build
bridges host-manager operations and reads host metadata through filesystem permissions.
Classic Snap executes host tools after removing paths inside `$SNAP` from discovery.

Flatpak uses `flatpak-spawn --host`, with access to `org.freedesktop.Flatpak` on
the session bus. It reads the host environment once, applies the same allowlist
as native execution, and clears the environment before invoking each host executable.
Pinned privileged paths stay pinned; missing host access fails closed. Sandboxed APT
metadata is read through the host's `dpkg-query` and `apt-cache`; native builds use
the bundled libapt helper. Flatpak unused-runtime cleanup is not offered until an
authoritative native preview can be implemented without a Python dependency. See the
[Flatpak command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn).

Snap uses classic confinement because its purpose requires discovering and invoking
the package managers already installed on the host. The Snap Store requires manual
approval for classic confinement. See [Snap confinement](https://snapcraft.io/docs/explanation/security/snap-confinement/).

## Environment and authorization

Host processes use absolute executables and argument arrays, with `/` as their
working directory. The executor clears inherited variables and supplies only
selected user/session variables and the host PATH, with the C locale for diagnostics.
It does not pass Qt library/plugin paths, loader injection variables, APT_CONFIG,
Python paths, Node options, or shell startup configuration. Empty and relative
PATH entries and executables resolving into APPDIR or `$SNAP` are excluded from discovery.
AppRun sets APPDIR for both entry points, including extracted launches.

User tools resolve from the invoking user's PATH and retain that user's HOME and
XDG user directories. System operations use typed requests validated by the
corresponding adapter.
Native distro managers, Flatpak system operations, repository changes, and firmware
updates retain their own authorization requirements. Homebrew and development tools
run as the invoking user. Frontends must stay unprivileged.

Authorized commands use a fixed system PATH, excluding user tool directories.
For a confirmed batch, the engine validates every typed operation and its saved
native preview before any write. When a root-owned `pkgdeck-host-runner` is
installed at `/usr/libexec/pkgdeck-host-runner`, PkgDeck starts it once through
the selected authorization method. The runner independently derives its allowed
commands from the typed operations, accepts only ordered, one-time requests for
those commands, and exits with the batch. It never accepts a shell script or an
arbitrary executable path. User-scoped commands stay in the unprivileged frontend.

The native system package can install the runner at that path. Classic Snap uses
its root-owned `$SNAP/usr/libexec/pkgdeck-host-runner`. A system-installed
Flatpak can use its own deployed runner after verifying its fixed system path,
root ownership, and non-writable ancestors on the host. A user-installed
Flatpak and an AppImage need a separately installed trusted host runner. If no
trusted runner is available, PkgDeck reports that it is using the existing
per-command authorization path, which can require more than one prompt.

For example, a single APT write without a trusted batch runner invokes the fixed
`/usr/bin/apt-get` path via either:

- `/usr/bin/pkexec --disable-internal-agent`, using an existing polkit agent/policy;
- `/usr/bin/sudo -n --`, requiring an existing grant and never prompting.

No passwords, policy files, or sudoers changes are installed by PkgDeck. The CI
fixture grants privileges only on its disposable hosted runner to exercise success
and denial. Interactive desktop authentication remains a release validation gate.
The pkexec 126/127 outcomes are reported as cancellation/denial, respectively;
see the [pkexec manual](https://polkit.pages.freedesktop.org/polkit/pkexec.1.html).

## Locks, cancellation, and failure

APT owns its normal dpkg/frontend locks. Requests use `DPkg::Lock::Timeout=0` so
contention is reported immediately. PkgDeck neither creates a competing lock
scheme nor removes native lock files. Installs and targeted upgrades use
`--no-remove`. APT Update all simulates `dist-upgrade` as an unprivileged host
read, displays installs and removals for approval, and re-simulates immediately
before the privileged write. A changed or incomplete plan stops the write.
CLI confirmation is documented in the
[CLI contract](cli.md).

Read commands have a deadline and bounded stdout/stderr capture (128 KiB per
stream by default). Excess output is drained and marked truncated. Cancellation
or timeout terminates their process group and reaps the direct child. A descendant
holding a pipe cannot indefinitely block completion.

Cancellation before authorization or a write starts prevents execution. A dismissed
batch prompt leaves every operation untouched. Once a
write starts, cancellation is **deferred** until the manager exits; the completion
records that request. Read deadlines do not forcibly interrupt writes. A native
manager can therefore keep a write pending until it finishes. Frontends must show
that a transaction is finishing instead of promising an immediate stop.

Authentication failure, lock contention, interruption, and other command failures
have separate errors. Signals and APT's interrupted-dpkg diagnostic require
inspection of native package state before retrying. A frontend crash or machine
shutdown may leave a transaction running or incomplete. There is no automatic
retry, rollback, lock deletion, or automatic `dpkg --configure -a` repair.

## Reproducing validation

Normal tests use synthetic executables and environments and never invoke an
APT write on the developer's host:

```sh
cargo test --locked -p pkgdeck-core -p pkd
cargo run --locked -p pkd -- doctor
```

Real authorization tests run in the x86_64 terminal CI job on a fresh GitHub-hosted
Ubuntu 26.04 runner. The guarded script checks native backend detection, rejection
of root frontends, denial for an unauthorized user, native lock contention, and
installation/removal of a synthetic APT package through both sudo and polkit.
`dpkg-query` and a fixture-owned file verify the resulting state. The script
removes its fixtures afterward, and GitHub discards the runner. The `apt-probe`
executable is never packaged.

Versioned APT and Homebrew lifecycles run in rootless Podman on both CI
architectures, with a controlled package repository and Homebrew tap. See
[local verification](development.md#local-verification-and-development-tools).
Synthetic boundary tests also run on both native CI architectures. Real ARM
authorization, an installed Flatpak host bridge, interactive auth-dialog
dismissal, and FUSE-based AppImage launching remain explicit release gates;
the hosted-runner test does not claim them.
