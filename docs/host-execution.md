# Host execution and authorization

Step 2 establishes the execution boundary in `pkgdeck-core`. `pkd doctor` reports
its runtime, executes a bounded, unprivileged host architecture probe, and locates
backend executables in the invoking user's host PATH. Detection does not imply
that a backend's package lifecycle is implemented. pip is deliberately omitted
until an explicit virtual environment can be selected.

## Format capabilities

| Format | Host reads/detection | APT / Homebrew writes | Validation |
| --- | --- | --- | --- |
| Native | Enabled | Enabled through the core and CLI | Synthetic process tests; real sudo/polkit and APT in a disposable Ubuntu x86_64 VM |
| AppImage | Enabled, outside the bundle | Enabled through the same native boundary | Extract-and-run AppImage doctor probe, GUI and terminal smoke tests; environment-isolation tests |
| Flatpak | **Disabled** | **Disabled** | Runtime detection fails closed before executable lookup or spawning; no host D-Bus permission is shipped |
| Snap | **Disabled** | **Disabled** | Strict confinement remains the packaging default; runtime detection fails closed |

The format gate is checked for reads, executable discovery, and authorized writes.
Snap and Flatpak markers take precedence over AppImage markers. Installed Flatpak
and Snap host operations are intentionally unavailable, even when an executable
with the same name exists inside their runtime.

Flatpak's future route is `flatpak-spawn --host`, which requires access to
`org.freedesktop.Flatpak` on the session bus. Before enabling it, an installed
bundle must prove host identity, host HOME/PATH resolution, permission denial,
authorization, and cancellation. Forwarding the sandbox's environment is not an
acceptable host environment. No fallback to in-sandbox APT is permitted. See the
[Flatpak command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn).

Snap's strict confinement does not provide this project with a tested route to
host system package managers. A separately reviewed host service or distribution
change would need its own installed-Snap tests. Classic confinement is not enabled
as a workaround. See [Snap confinement](https://snapcraft.io/docs/explanation/security/snap-confinement/).

## Environment and authorization

Host processes use absolute executables and argument arrays, with `/` as their
working directory. The executor clears inherited variables and supplies only
selected user/session variables and the host PATH, with the C locale for diagnostics.
It does not pass Qt library/plugin paths, loader injection variables, APT_CONFIG,
Python paths, Node options, or shell startup configuration. Empty and relative
PATH entries and executables resolving into APPDIR are excluded from discovery.
AppRun sets APPDIR for both entry points, including extracted launches.

User tools resolve from the invoking user's PATH and retain that user's HOME and
XDG user directories. The only authorized write API is a typed APT request for a
validated Debian package name and optional architecture, or metadata refresh. Homebrew and development tools have no elevation
route. Frontends must stay unprivileged; the APT API rejects a root caller.

Authorized commands use a fixed system PATH, excluding user tool directories.
The adapter invokes the fixed `/usr/bin/apt-get` path via either:

- `/usr/bin/pkexec --disable-internal-agent`, using an existing polkit agent/policy;
- `/usr/bin/sudo -n --`, requiring an existing grant and never prompting.

No passwords, policy files, or sudoers changes are installed by PkgDeck. The VM
fixture grants privileges only inside its disposable guest to exercise success
and denial. Interactive desktop authentication remains a release validation gate.
The pkexec 126/127 outcomes are reported as cancellation/denial, respectively;
see the [pkexec manual](https://polkit.pages.freedesktop.org/polkit/pkexec.1.html).

## Locks, cancellation, and failure

APT owns its normal dpkg/frontend locks. Requests use `DPkg::Lock::Timeout=0` so
contention is reported immediately. PkgDeck neither creates a competing lock
scheme nor removes native lock files. Install and upgrade use `--no-remove`;
APT handles dependencies and transactions. CLI confirmation is documented in the
[CLI contract](cli.md).

Read commands have a deadline and bounded stdout/stderr capture (128 KiB per
stream by default). Excess output is drained and marked truncated. Cancellation
or timeout terminates their process group and reaps the direct child. A descendant
holding a pipe cannot indefinitely block completion.

Cancellation before a write starts prevents authorization and execution. Once a
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

Real authorization tests require QEMU, qemu-img, genisoimage, network access for
Ubuntu package downloads, and an x86_64 build host. KVM is used when accessible;
otherwise QEMU uses software emulation:

```sh
cargo build --locked -p pkgdeck-core --example apt-probe
cargo build --locked -p pkd
python3 scripts/test-host-vm.py
```

The launcher verifies the SHA-256 of a dated Ubuntu 26.04 cloud image, creates a
disposable overlay, and shares the checkout read-only. All package and policy
changes occur inside the guest. It checks native backend detection, rejection of root frontends, denial for an unauthorized user, native
lock contention, and installation/removal of `pkgdeck-fixture` through both sudo
and polkit. `dpkg-query` and a fixture-owned file verify the resulting state.
The guest also exercises CLI versioned APT and Homebrew lifecycles through
install, refresh, update detection, upgrade, and removal, checking native state.
Homebrew uses a controlled tap and the standard Linux prefix as an unprivileged
user. The VM CPU exposes SSSE3, required by Homebrew on x86_64.
Dependencies are prepared once in a verified cached VM image. Every test uses a
fresh overlay, which is deleted on completion or failure. Logs stream live and
remain in `build/host-vm/logs/`. See [prepared guests and Podman](development.md#prepared-qemu-guests)
for cache invalidation and the faster container lifecycle checks. The development `apt-probe` executable is never packaged.

The VM test runs in a separate x86_64 CI job. Synthetic boundary tests run on both
native CI architectures. Real ARM authorization, installed Flatpak/Snap host
bridges, interactive auth-dialog dismissal, and FUSE-based AppImage launching
remain explicit release gates; they are not claimed by the local VM result.
