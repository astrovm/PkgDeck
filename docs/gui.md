# Graphical package browser

`pkgdeck` opens the Qt/Kirigami frontend. It uses the same shared engine and
host authorization boundary as the terminal frontend.

![Implemented GUI](screenshots/pkgdeck-gui-live.png)

[Light appearance](screenshots/pkgdeck-gui-light-live.png) ·
[Compact layout](screenshots/pkgdeck-gui-compact-live.png)

Captures show real local packages in the true-black dark theme.
The default appearance follows the system; Settings also offers explicit Dark and Light modes.

The sidebar provides Search, Installed, Updates, Sources, Settings, and
About. Sources reports actual source availability on the current computer;
it does not invent recommendations or combine matching names across sources.
Search submits on Enter or the Search button. Results use a virtualized ListView,
ranked best-match-first (exact name, name prefix, name substring, then summary
matches), with the selected package's description, scope, homepage, and dependencies below.
Unverifiable offers sort last: a row with neither an installed nor a candidate
version (for example a dev backend guessing an uninstalled name) never buries
a real package. Submitting a search clears any column sort so the best match
is always first.
The VERSION column carries the state: a bare candidate means not installed,
`· installed` marks installed packages outside the Installed view, and `→`
marks an available update. Installed rows show only their version because the
view already establishes their state.
Matching names remain separate source/architecture/scope identities.
Installed packages related to an installation from another manager show a
distinct "Related install: …" badge on the row, tooltip, and details panel. This
is intentionally not described as an exact duplicate: package names and package
boundaries can differ between managers. Two
local-only signals feed the grouping: the AppStream component id (APT's
DEP-11 data, Flatpak app ids, snap desktop entries, AppImage desktop
entries) and the upstream homepage (APT control data, Homebrew formulae,
and dev-tool manifests), normalized and matched exactly. Grouping is
display-only; installs, removals, and upgrades still address one exact
backend identity, and remote catalog entries never join a group. The
Installed view's Related installs checkbox lists just those packages:

Click a column header to sort ascending, again for descending (▲▼); drag the
header gutter to resize the name and version columns. Headers are Tab stops:
Space or Return sorts without a mouse. Sorting is display-only:
selection and actions map the visible row back to backend order. Column
widths and the active sort persist across restarts like the source filter.

Install, Remove, Upgrade, and Refresh source operate on the selected identity.
Updates checks every row by default. Uncheck rows to narrow the upgrade, or
use Select none to start empty and Select all to re-check everything; the single **Upgrade** button (Ctrl+Shift+U)
reads **Upgrade all** while everything is checked and **Upgrade selected**
otherwise. The all path batches per backend; the subset path confirms exactly
the checked identities, which re-resolve against the current rows, so entries
that moved on or vanished while streaming are skipped, never guessed. Upgrade
all is disabled when there are no upgrades or any source query failed; a hint
beside the button names the reason while it is unavailable. The batch reports
every result, including partial failures and cancellation. Completed upgrades
are not rolled back; cancellation skips remaining writes once the active
native transaction finishes. Reload reads the remaining updates and re-checks
every row.

![Upgrade all](screenshots/pkgdeck-updates-live.png)

Each write requires confirmation, with No focused initially. Refresh changes
source metadata only. Successful writes clear stale results; use Reload to read
current state. Failures stay as rows in the results table; selecting a failed
row shows the underlying manager error with a remediation hint, without
re-querying. Native dependency changes can accompany package operations.

Queries and writes execute on a Rust worker. The GUI polls a message channel and
updates Qt properties on its own thread. Search, Installed, and Updates
results stream in per backend, so rows appear while slow sources still
query; the selection follows the
same package identity across partials. Upgrade availability still waits for
the terminal report with complete failures. Hovering a package row shows its full
untruncated versions and summary. Finished views are cached, so switching
sections shows the last results instantly; only the first visit, a source
change, or Reload queries native managers, and writes invalidate every cached
view. Switching sections or the selected row during
a query cancels the in-flight read and starts the new one; native writes keep
the lock until they finish. An indeterminate spinner in the results table
replaces invented percentages. Cancel interrupts reads; an already-running
native write finishes safely under its manager's lock. Closing a busy window
requests cancellation and keeps it open until completion.

Settings persist appearance, source filter, and authorization preference through Qt's
per-user settings. The source filter is also available as a checklist in the
header, next to the current view name: unchecking hides a source from every
query, which is how backends you never use stay silent. The last checked
source stays enabled so queries can never select nothing; the choice persists
across restarts. The Installed view has its own filter
field that narrows the loaded rows as you type, without a new native query.
Enter jumps to the first match. The default GUI authorization uses the host polkit agent.
Existing sudo credentials are also supported; passwords are never collected by
PkgDeck. Homebrew and the development managers (Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems) remain unprivileged. `pkgdeck --from apt|homebrew|cargo|npm|pnpm|bun|pip|pipx|uv|composer|gem --auth
sudo|polkit` overrides the saved settings for the current session.

The interface uses a consistent surface, border, and accent palette in both light
and dark mode. Text labels accompany navigation, and package metadata remains
plain text. Narrow windows replace the sidebar with a navigation selector and
compact result rows. Only applicable package actions are shown. Details remain
selectable and scrollable.

Results are virtualized. Up to 128 detail records are cached for the current
result snapshot, so revisiting a package avoids launching another native query.
Reloading, changing the source/view query, and successful writes invalidate this
cache. Background polling stops when work finishes.

| Shortcut | Action |
| --- | --- |
| Ctrl+F | Open Search and focus its field |
| Ctrl+L | Focus package/source results |
| Up/Down | Select a result and load its details |
| Down in the search field | Jump to the results and select the first row |
| PageUp/PageDown/Home/End | Move the selection in larger steps or to either end |
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 | Search / Installed / Updates / Sources |
| Ctrl+I / Ctrl+D / Ctrl+U | Propose install / remove / upgrade |
| Ctrl+Shift+U in Updates | Confirm the checked upgrades (all by default) |
| Ctrl+M | Propose metadata refresh for the selected source |
| Ctrl+R | Reload the current view |
| Alt+Y / Alt+N in confirmation | Confirm / reject |
| Escape during work | Request cancellation |
| Ctrl+Q | Close, waiting safely if work is active |

## Bundled icons

Navigation, package actions, source rows, details, and the sidebar heart use original vector artwork
in `DeckIcon.qml`. CXX-Qt embeds this component in the application's Qt resources,
so native, AppImage, Flatpak, and Snap builds carry the same icons inside the
executable. They use the already-required Qt Quick Canvas renderer: no runtime downloads, host icon theme, or icon font is required. Source symbols
are generic archive/mug illustrations, not third-party logos. The original artwork follows
the repository's MIT license. The sidebar's GitHub mark is bundled from
[GitHub Octicons](https://github.com/primer/octicons) with its MIT license;
it uses the Qt SVG plugin already included in the packages. The sidebar links to
https://github.com/astrovm/PkgDeck and displays “Made with ♥ by astro”.

Qt Quick pixel tests render every icon in two colors with empty XDG icon-data
directories. Packaged GUI smoke tests use the same isolated data directories.
Icons follow the light/dark palette and disabled states. Text labels remain the
accessible names; decorative icons are ignored by accessibility tools.

Package rows and the details header additionally show the application's own
icon where the host already has one: Snap metadata, Flatpak exports, and APT
desktop entries are read from local files only, never downloaded. Results
without a local icon keep the generic source symbol.

## Local verification

`scripts/verify.sh full --only tests --engine podman` runs Qt Quick tests against the actual Browser component,
Rust worker tests, and a real-window lifecycle using a synthetic Homebrew
executable. The real-window test creates a private Xvfb display; it does not use
the user's display or package database. It verifies native fixture state after
install, refresh, upgrade, and removal, and exercises resize and read cancellation.
Qt Quick tests cover keyboard navigation, exact confirmation, source views,
loading/errors, the source checklist, Updates multi-select, column
sort/resize with backend index mapping, the same-application badge,
best-match search ranking, and the live Installed filter. Xvfb, xdotool, and fonts are included in
the development image and desktop CI dependencies. See
[development](development.md#running-tests) for focused reruns and native setup.

To check the staged release bundle against real managers using the native
development prerequisites:

```sh
scripts/verify.sh full
source scripts/dev-env.sh
scripts/bundle.sh
PKGDECK_FRONTEND=gui PKGDECK_BINARY_DIR="$PWD/build/AppDir/usr/bin" \
  scripts/container.sh lifecycle
```

This mounts AppDir read-only into a disposable rootless container, opens its GUI
on a private display, sends keyboard actions, and verifies synthetic 1.0 → 2.0
APT/Homebrew packages using the underlying managers. The application uses its
bundled Qt/Kirigami libraries; host commands use the sanitized host boundary.
Desktop CI runs this same packaged GUI check on x86_64 and aarch64.

The native/AppImage execution path is enabled. Flatpak and Snap host operations
remain explicitly disabled; their packaged GUI smoke checks do not claim a native
package lifecycle. Polkit and manager-lock behavior retain the shared VM tests.
Installed-format distribution validation remains a later release gate.
