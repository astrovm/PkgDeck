# Host access and authorization

This page explains how PkgDeck runs your package managers and asks for
permission. The shared code lives in `pkgdeck-core`.

Run `pkd doctor` to see how PkgDeck is running, your system's architecture,
and which package managers it found on your `PATH`. Finding a package manager
doesn't mean every action is supported. Each one declares what it can do.

## Package formats

PkgDeck always manages the package managers installed on your system, even
when PkgDeck itself is packaged.

| Format | How it reaches the system | Tested by |
| --- | --- | --- |
| Native | Directly | Fake-process tests; real sudo/polkit and APT on a throwaway GitHub-hosted Ubuntu runner |
| AppImage | Directly, ignoring tools inside the AppImage | `pkd doctor` in the extracted AppImage, GUI and terminal tests, environment tests |
| Flatpak | Through `flatpak-spawn --host` | Real APT reads, a test Homebrew, and Flatpak install/remove |
| Snap (classic) | Directly, ignoring tools inside `$SNAP` | Isolation tests and installed-package tests in CI |

The same rules apply to reads, finding programs, and changes. If both Snap or
Flatpak and AppImage markers are present, Snap or Flatpak wins.

### Flatpak

The Flatpak runs host commands with `flatpak-spawn --host`, which needs access
to `org.freedesktop.Flatpak` on the session bus. It reads your environment
once, keeps only the allowed variables, and starts each command with a clean
environment. If host access is missing, commands fail instead of falling back
to something else. Inside the Flatpak, APT data comes from the host's
`dpkg-query` and `apt-cache`. Native builds use the bundled APT reader. Unused
runtime cleanup isn't offered yet. See the
[Flatpak command reference](https://docs.flatpak.org/en/latest/flatpak-command-reference.html#flatpak-spawn).

### Snap

The Snap uses classic confinement because PkgDeck needs to run the package
managers already on your system. The Snap Store must manually approve classic
confinement. See
[Snap confinement](https://snapcraft.io/docs/explanation/security/snap-confinement/).

## Environment

Every command runs by full path, with a list of arguments (never through a
shell), from `/`:

- PkgDeck clears the environment and passes only selected user and session
  variables, your `PATH`, and the C locale.
- It never passes Qt paths, library injection variables, `APT_CONFIG`, Python
  paths, Node options, or shell startup files.
- Empty or relative `PATH` entries are ignored, as are programs inside the
  AppImage or `$SNAP`.
- AppRun sets `APPDIR` for both the app and `pkd`, including extracted
  AppImages.

User tools like Homebrew and developer package managers run as you, with your
`PATH`, home folder, and XDG folders. The app and CLI never run as root.

## Asking for permission

System package managers, Flatpak system installs, repository changes, and
firmware updates need admin rights. Those commands use a fixed system `PATH`
without your personal tool folders.

### One prompt per batch

When you confirm a batch of changes, PkgDeck:

1. Checks every change and its saved preview before changing anything.
2. Starts its bundled helper once through your chosen method (sudo or
   polkit), so you only authorize once.
3. The helper works out which commands are allowed from the confirmed
   changes. It only accepts those commands, in order, once each, and exits
   when the batch ends. It never accepts a shell script or any other program.

User-level commands never go through the helper.

### Where the helper lives

- Native, classic Snap, and Linux Homebrew: next to the PkgDeck executables.
- AppImage: started through AppRun, so the elevated process can mount the
  AppImage.
- Flatpak (user or system): inside its own install, through
  `flatpak-spawn --host`.

If the helper is missing, PkgDeck says so and authorizes each command
separately, which may mean more than one prompt. For example, one APT change
runs `/usr/bin/apt-get` through either:

- `/usr/bin/pkexec --disable-internal-agent`, using your desktop's polkit
  prompt, or
- `/usr/bin/sudo -n --`, which only works with an existing sudo login or
  password-free rule, and never prompts.

PkgDeck never installs passwords, polkit policies, or sudoers rules. The CI
test only grants permissions on a throwaway runner. Desktop password prompts
are checked by hand before each release. pkexec exit codes 126 and 127 are
reported as "cancelled" and "denied". See the
[pkexec manual](https://polkit.pages.freedesktop.org/polkit/pkexec.1.html).

## Locks, cancelling, and failures

### APT locks

APT uses its normal locks. PkgDeck sets
`DPkg::Lock::Timeout=0`, so if APT is busy you're told right away. PkgDeck
never creates its own locks or deletes APT's lock files.

### APT safety

Installs and single-package upgrades use `--no-remove`. For
**Update all**, PkgDeck runs `dist-upgrade` as a dry run without admin rights
and shows the installs and removals. It runs the dry run again right before
the real upgrade. If the plan changed or is incomplete, it stops. See the
[CLI guide](cli.md) for how the CLI confirms changes.

### Reads

Reads have a time limit and keep up to 128 KiB of output per stream. Extra
output is discarded and marked as cut off. When a read is cancelled or times
out, its whole process group is stopped, so a leftover child process can't
hang PkgDeck.

### Cancelling

- Cancelling before authorization or before a change starts stops it
  completely.
- Closing the password prompt leaves everything untouched.
- Once a package manager starts making changes, PkgDeck waits for it to
  finish. The result notes that you asked to cancel. The app and CLI show that
  the change is finishing, instead of claiming it stopped.

### Failures

Permission errors, locks, interruptions, and other failures are
reported separately. If a package manager was interrupted, for example by
a signal or an interrupted dpkg run, check your system before trying again. If
PkgDeck crashes or the computer shuts down during a change, the change may be
unfinished. PkgDeck never retries, undoes changes, deletes locks, or runs
`dpkg --configure -a` for you.

## Testing

Regular tests use fake programs and never run APT changes on your computer:

```sh
cargo test --locked -p pkgdeck-core -p pkd
cargo run --locked -p pkd -- doctor
```

Real authorization tests run in CI on a fresh GitHub-hosted Ubuntu 26.04
x86_64 runner. They check detection, that PkgDeck refuses to run as root,
that unauthorized users are denied, lock handling, and installing and
removing a test APT package through sudo and polkit. Results are verified
with `dpkg-query`. The runner is discarded afterward. The `apt-probe` test tool
is never packaged.

APT and Homebrew install/remove tests run in rootless Podman on both
architectures, using a local test repository and Homebrew tap. See
[development](development.md#podman-details).

These are checked by hand before each release, not in CI: real ARM
authorization, the installed Flatpak host bridge, closing the password
prompt, and AppImages mounted with FUSE.
