# Graphical interface

Launch `pkgdeck`. The sidebar contains Search, Installed, Updates, Sources,
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

Installed supports filtering and a **Duplicate installs** view. AppStream identity
and upstream homepage metadata associate related installations; grouping does not
merge packages or change what an action targets.

Click column headings to sort; drag the name and version dividers to resize.
Source filters and column preferences persist between sessions. **Reload** reads
fresh package state.

## Updates

Updates includes package updates and, when `fwupdmgr` is available, device firmware.
Use a row's arrow to update one item, or check rows and choose **Update selected**.
With everything checked, the button reads **Update all**.

Version arrows show installed → candidate versions when the source supplies both.
Some Flatpak runtimes expose branches or commit identifiers instead of release
versions. PkgDeck does not invent version numbers.

Review the confirmation before applying changes. Firmware confirmations include
power and restart requirements; PkgDeck does not reboot automatically. Failed
sources are shown explicitly and block Update all until the result is complete.

Cancel stops pending work. If a native transaction has already started, it finishes
before cancellation takes effect. Successful changes are not rolled back after a
later failure. Reload to inspect the resulting state.

## Repositories and firmware

Open **Sources → Repositories** to inspect configured repositories.

![Repository management in PkgDeck](screenshots/repositories.png)

Flatpak repositories can be added, removed, enabled, disabled, and prioritized,
separately for User and System installations. Firmware remotes can be toggled.
APT's editor button opens the native Software Sources tool. Repository changes
use the manager's normal signature verification and authorization.

The source filter controls which managers PkgDeck queries; it does not enable or
disable repositories. Refresh sources updates metadata for selected managers.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 | Search / Installed / Updates / Sources |
| Ctrl+F | Focus the search or filter field |
| Ctrl+R | Reload |
| Ctrl+Shift+U | Update checked packages |
| Ctrl+Q | Close; finish an active native transaction first |

Tab navigates controls. Row actions and icon buttons have accessible names;
column headings also support Space or Return to sort.

## Availability

The GUI runs natively on Linux and is packaged with the CLI by Homebrew on macOS.
Available sources depend on the platform and installed managers. Experimental
Flatpak and Snap builds cannot currently manage host packages. See
[distribution](distribution.md) and [authorization](host-execution.md).
