# Graphical interface

Launch `pkgdeck`. The sidebar contains Search, Installed, Updates, Clean, Sources,
and Settings. Activity is always available in the header. Settings includes
appearance, authorization, version, GitHub, and keyboard shortcuts.

## Browse and compare

Search by app or package name. Exact matches appear before plugins and libraries,
with variants from different managers together. Each row retains its own source,
architecture, and installation scope. For Flatpak, choose the User or System row
to target that installation.

The row button installs, removes, or updates that exact package. Click the row
for any additional description, screenshots, publisher, license, homepage, or
dependencies supplied by that source. Details are optional and close with the
× button. App names, icons, and screenshots depend on source metadata; packages
without desktop metadata keep their native names.

## Open packages and sources

On Search, choose **Add…**, drop one file onto the window, or pass a path or URL to
`pkgdeck`. Package files: `.AppImage`, `.deb`, `.rpm`, `.pkg.tar.zst` (also
`.xz`, `.gz`, `.bz2`, `.lz4`), `.flatpak`, `.flatpakref`, and `.snap` with a
matching `.assert`. Repository files: `.flatpakrepo`, `.repo`, `.sources`,
`.list`, and openSUSE `.ymp`. Direct HTTPS links to these files and
`flatpak+https://…flatpakref` links work too. A second launch forwards its
input to an already open PkgDeck window. Opening shows a confirmation preview;
it never installs a file by itself.

AppImages are inspected as Type 2 ELF files without execution. The preview
shows the managed destination, executable copy, desktop entry, and whether
embedded update metadata is present. Import keeps the original file. Local
Debian packages show APT's simulated package changes before system
authorization. For Flatpak references, choose User or System in the confirmation.
The preview shows the exact app, repository, signing key presence, and any runtime
repository named in the reference.
Flatpak may discover additional runtimes during installation.
Native package managers inspect RPM, Arch, and Snap archives. Flatpak checks
bundle contents during install. Repository files are checked again before
they are added. `.ymp` opens the native openSUSE installer after confirmation.

Installed supports filtering and a **Duplicate installs** view. AppStream identity
and upstream homepage metadata associate related installations; grouping does not
merge packages or change what an action targets.

Click column headings to sort; drag the name and version dividers to resize.
Use the source picker to filter the current package page. Enable managers on
Sources; that choice persists between sessions. The picker shows unavailable
managers and why they cannot be used. **Reload** reads fresh package state.

Docker daemon images and rootless Podman images are separate sources. Rows show
tags, digests, size, age, immutable short ID, and storage scope. A tagged image
can be pulled again or removed after confirmation; dangling images can only be
removed. To pull a new registry reference, select only Docker or Podman and
search for the exact tagged reference.

## Clean

Clean lists available maintenance tasks. Inspect one task or choose **Clean all**;
writes require confirmation. Sources without cleanup support are omitted. Failed
checks appear separately. APT checks unused dependencies without authentication
and counts cached downloads without entering its protected directory. APT chooses
obsolete downloads when cleanup runs, then asks for system authentication.

Views identify cached results and show them immediately for up to 60 seconds.
Expired results stay visible while a new check runs. Failed source checks appear
in a separate notice with details and a retry for that source. Installed and
Updates share one inventory. The source picker checks availability on demand
without occupying the search worker. Installed, Updates, and Clean preload
after leaving Search. Foreground requests take priority. Package and repository
changes invalidate the cache. Cleanup always checks the confirmed plan again
before execution.

## Updates

Updates includes package updates and, when `fwupdmgr` is available, device firmware.
Use a row's arrow to update one item, or check rows and choose **Update selected**.
With everything checked, the button reads **Update all**.

Version arrows show installed → candidate versions when the source supplies both.
Some Flatpak runtimes expose branches or commit identifiers instead of release
versions. PkgDeck does not invent version numbers.

Review the target, source, scope, and available native plan before applying
changes. The confirmation button names the action; Cancel has focus by default.
APT provides a dry run for individual package changes and rechecks it before
writing. Other managers may not provide a transaction preview. Firmware confirmations include
power and restart requirements; PkgDeck does not reboot automatically. Failed
sources are shown explicitly and block Update all until the result is complete.
For APT, Update all previews the `dist-upgrade` transaction. Planned installs
and removals appear at the top of the confirmation, and a changed plan stops
the update before authorization.

Cancel stops pending work. If a native transaction has already started, it finishes
before cancellation takes effect. Successful changes are not rolled back after a
later failure. Reload to inspect the resulting state.

## Standalone CLI tools

Supported standalone Codex, Claude Code, Grok, and OpenCode installations appear
in Installed and Updates. They use their upstream updaters without administrator
privileges. Installations managed by npm or Homebrew stay with that manager.
These sources update existing tools; they do not install or remove them. See
[supported layouts](cli.md#standalone-cli-tools).

## Repositories and firmware

Open **Sources → Repositories** to inspect configured repositories.

![Repository management in PkgDeck](screenshots/repositories.png)

Flatpak repositories can be added, removed, enabled, disabled, and prioritized,
separately for User and System installations when each installation can be
changed. A system installation that can only be read stays visible without edit
controls. Firmware remotes can be toggled.
On APT systems with a supported editor and a graphical authorization prompt,
**Edit APT sources** opens the distro's Software Sources tool. DNF and Zypper
repositories can be viewed, and signed repository files can be opened for import.
PkgDeck does not edit Pacman repositories or Homebrew taps. Controls without an
available manager or working equivalent are hidden. Repository changes use the
manager's normal signature verification and authorization.

| Platform or manager | Repository controls |
| --- | --- |
| APT | View sources; open Software Sources when its editor is available |
| DNF and Zypper | View repositories; import reviewed `.repo` files |
| Pacman | No repository editor; import local package archives |
| Flatpak | Manage available User and System installations |
| Firmware on Linux | Enable or disable available remotes |
| macOS with Homebrew | No repository editor; manage formulae and casks in the package views |

The Add file picker only lists formats for managers available on this computer.
Linux AppImages, desktop login autostart, and system authorization settings are
omitted on macOS.

## Background update notifications

Enable **Background checks** in Settings to check selected package sources while
PkgDeck is running. The first check starts about 30 seconds after launch; later
checks run no more often than every 30 minutes. Checks wait while the computer is
offline, on a metered connection, or busy with another package operation. On
Linux, **Start in background at login** keeps these checks running after login.
Click the tray icon to hide or show the window. Its menu also offers Check now
and Quit; clicking an update notification opens Updates.

PkgDeck notifies you when a successful check finds updates it has not notified
you about before, including on the first check. It remembers notified updates
across restarts and does not alert when updates disappear. A failed source does
not prevent alerts for sources that completed successfully. Settings shows the
last check, the count from successful sources, any source errors, and whether
the desktop tray supports notifications. **Test notification** sends a sample
message when background checks and tray notifications are available.

The source picker filters one package page; Sources controls which managers PkgDeck
queries. Neither setting enables or disables repositories. Refresh sources
updates metadata for selected managers.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 / Ctrl+5 | Search / Installed / Updates / Clean / Sources |
| Ctrl+F | Search field in Search, package filter in Installed, source picker in Updates/Clean; other pages open Search |
| Ctrl+R | Reload |
| Ctrl+Shift+U | Update checked packages |
| Ctrl+Q | Close; finish an active native transaction first |

Tab navigates controls. Row actions and icon buttons have accessible names;
column headings also support Space or Return to sort.

## Availability

The GUI runs natively on Linux and is packaged with the CLI by Homebrew on macOS.
Available sources depend on the platform and installed managers. The Flatpak and classic Snap builds manage host packages. See
[distribution](distribution.md) and [authorization](host-execution.md).
