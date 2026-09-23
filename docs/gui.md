# Graphical interface

Launch `pkgdeck`. The sidebar contains Search, Installed, Updates, Clean, Sources,
Settings, and About. Settings offers system, dark, and light appearance, reduced
motion, and authorization preferences.

## Browse and compare

Search by app or package name. Exact matches appear before plugins and libraries,
with variants from different managers together. Each row retains its own source,
architecture, and installation scope. For Flatpak, choose the User or System row
to target that installation.

The row button installs, removes, or updates that exact package. Click the row for
its description, native identifier, and available screenshots. Details are optional
and close with the × button. App names, icons, and screenshots depend on source
metadata; packages without desktop metadata keep their native names.

## Open installation files

Choose **Open…**, drop one file onto the window, or pass a path or URL to
`pkgdeck`. Supported inputs are local `.AppImage`, `.deb`, and `.flatpakref`
files, plus `flatpak+https://…flatpakref` links. A second launch forwards its
input to an already open PkgDeck window. Opening shows a confirmation preview;
it never installs or launches a file by itself.

AppImages are inspected as Type 2 ELF files without execution. The preview
shows the managed destination, executable copy, desktop entry, and whether
embedded update metadata is present. Import keeps the original file. Local
Debian packages show APT's simulated package changes before system
authorization. Flatpak references show the exact app, user scope, repository,
signing key presence, and any runtime repository named in the reference.
Flatpak may discover additional runtimes during installation.

Installed supports filtering and a **Duplicate installs** view. AppStream identity
and upstream homepage metadata associate related installations; grouping does not
merge packages or change what an action targets.

Click column headings to sort; drag the name and version dividers to resize.
Use the source picker to filter the current page. Enable managers separately in
Settings; that choice persists between sessions. The picker shows unavailable
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
in a separate notice with details and a retry for that source. Installed and Updates share one inventory.
Sources, Installed, Updates, and Clean preload sequentially in the background;
foreground requests take priority. Package and repository changes invalidate the
cache. Cleanup always checks the confirmed plan again before execution.

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
separately for User and System installations. Firmware remotes can be toggled.
APT's editor button opens the native Software Sources tool. Repository changes
use the manager's normal signature verification and authorization.

The source picker filters one page; Settings controls which managers PkgDeck
queries. Neither setting enables or disables repositories. Refresh sources
updates metadata for selected managers.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 / Ctrl+5 | Search / Installed / Updates / Clean / Sources |
| Ctrl+F | Search field in Search, package filter in Installed, source picker in Updates/Clean/Sources; other pages open Search |
| Ctrl+R | Reload |
| Ctrl+Shift+U | Update checked packages |
| Ctrl+Q | Close; finish an active native transaction first |

Tab navigates controls. Row actions and icon buttons have accessible names;
column headings also support Space or Return to sort.

## Availability

The GUI runs natively on Linux and is packaged with the CLI by Homebrew on macOS.
Available sources depend on the platform and installed managers. The Flatpak and classic Snap builds manage host packages. See
[distribution](distribution.md) and [authorization](host-execution.md).
