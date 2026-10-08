# PkgDeck GUI: functional spec for the egui port

Source of truth: `crates/pkgdeck/qml/*.qml`, `crates/pkgdeck/native/*.cpp`,
`crates/pkgdeck/src/main.rs`, and the controller in
`crates/pkgdeck-app/src/controller.rs` (`ffi::PackageController`).
Everything below describes what the Qt/QML window does today, so an egui
front end can match it without reading QML.

Conventions:
- `controller.x` = a property getter on `ffi::PackageController` (all JSON
  properties are `QString` holding JSON text; parse with `serde_json`).
- QML calls camelCase names (`backend.openInput`); the Rust methods are
  snake_case (`open_input`). Both are listed.
- "index" arguments are always the **raw index into the controller's last
  `rows` array** (not the visible, sorted/filtered index). See §4.4.
- Row identity (used everywhere) =
  `JSON.stringify([row.source, row.name, row.architecture, row.remote || null, row.scope, row.reference || null])`.
  `scope` is the serialized `Scope`: `"system"`, `{"user":{"uid":N}}` or
  `{"environment":{"path":"..."}}`.

---

## 1. Window, pages and navigation

### 1.1 Window
- Title `PkgDeck`. Default size 1100×760. Minimum 360×400.
- Background color `canvas`. Layout: **sidebar | page content**.
- Visible at start unless `--background` was passed AND `backgroundMode`
  setting is on AND a tray is available.
- Drag-and-drop: exactly one URL dropped anywhere → `openExternalInput(url)`
  (§5.10). More than one URL is rejected.

### 1.2 Sidebar
- Width: `sidebarWidth` setting (default 212). Clamped to
  `[sidebarMinimumWidth, sidebarMaximumWidth]` where
  - `sidebarMinimumWidth = max(180, 28+10+titleWidth+4+28, footerWidth+4+28)`
  - `sidebarMaximumWidth = max(min, min(360, windowWidth - 748))`
- **Rail mode** (icons only, width 64) when window width < 820 OR saved
  width < 150 (railThreshold). Switching rail↔full by drag slides (120 ms);
  by window resize it snaps.
- Content (margins 14, rail 10; spacing 8):
  1. Brand row: 28 px logo (`logo.svg`) + "PkgDeck" (bold, 1.55× font).
     Title hidden in rail. Vertically aligned with the page heading.
  2. 18 px gap.
  3. Nav entries (spacing 4), each a "navigation" ActionButton:
     | Page | Icon | Shortcut |
     | --- | --- | --- |
     | Search | `search` | Ctrl+1 |
     | Installed | `installed` | Ctrl+2 |
     | Updates | `updates` | Ctrl+3 |
     | Clean | `remove` | Ctrl+4 |
     | Sources | `sources` | Ctrl+5 |
     | Settings | `settings` | Ctrl+, / Ctrl+6 |
     - Rail: icon only, name as tooltip. Click → `openView(name)`.
     - Current page: a `selection`-colored pill slides (y animation 120 ms)
       behind the entry, with a 3 px accent bar on its left (full mode only).
       Current entry text DemiBold, icon accent.
  4. Filler.
  5. Footer (full mode only): "Made with" ♥ (`#e34b5f`, 14 px) "by astro",
     muted, 0.9× font.
- Right edge: 1 px `line`. Resize handle 12 px wide on the right edge
  (only when window ≥ 820): SplitH cursor, 2 px accent line at 0.6 opacity on
  hover/drag/focus. Drag sets `sidebarWidth` (dragged < 150 → 64 = rail).
  Double-click → 212. Keyboard (focusable via Tab): Left/Right ±16,
  Home → 212.

### 1.3 Page content column
- Margins: 28 (window ≥ 820) else 12. Spacing 14. Fades in from 0.35 opacity
  over 120 ms on every page switch.
- Disabled (no input) while an opened file/link page is shown (§3.2).
- Children, top to bottom (each only when its condition holds):
  1. Header row (§1.4)
  2. Search field (Search page)
  3. Empty Search prompt (Search, empty query, no rows, not opening)
  4. "Opening…" row (while `openingInput`): spinner + "Opening…" + **Cancel**
     → `controller.cancel()`
  5. Operation progress line (§6.2)
  6. Installed filters row (Installed page) (§2.2)
  7. Settings page (Settings) (§2.6)
  8. Notice banner (§5.8)
  9. Source-failure banner (§5.9)
  10. Results card (§4)
  11. Details panel for non-package rows (§3.1)
  12. App page for an opened row (§3.2), with its top-edge resize handle
  13. Page actions row (§2.x: Clean all, Updates actions, Repositories,
      Refresh sources)
  14. Filler (list pages, when the list doesn't fill the height)

### 1.4 Page header row
- Heading: current page name, bold 1.6× font, elided.
- **Install from file…** (`package` icon; icon-only with tooltip
  "Install from a file or link" when page width < 600). Visible on Search
  and Sources when `supportedFilePatterns()` is non-empty (§5.10).
  Disabled while `writing`. Click: `controller.check_sources()` then open the
  "Install from a file or link" dialog (§5.6).
- **Activity** button (`activity` icon). Label "Working" while `writing`,
  else "Activity" (icon-only under 600 px). Icon accent while writing.
  Badge (accent pill, 18 px high, bold caption text, top-right, overshoot
  pop-in) shows `queuedCount + (writing ? 1 : 0)` when > 0, where
  `queuedCount` = activity rows with `state == "queued"`. Click toggles the
  Activity drawer (§2.7).
- **Filter sources** button (`filter` icon) on Search, Installed, Updates,
  Clean. Label: `sourceSummary()` if this page has a filter, else
  "Filter sources". Icon accent when filtered. Width clamp 160..240 (icon
  only under 600 px). Click toggles the source picker popup (§5.7).

### 1.5 `openView(view)`
- `"Activity"`: open the drawer, call `controller.refresh_activity()`, stop.
- Otherwise close the drawer; if the page changes: drop retained rows and
  green "completed" highlights. Always: `queryDirty=false`,
  `selectedIdentity=null`, Updates unchecked set cleared, list selection -1.
- Search: reload only if the page changed; focus the search field.
- Installed/Updates/Clean/Sources: `reload()` (§4.2).
- Settings: nothing loads.

---

## 2. Pages

### 2.1 Search
- Search field: placeholder "Search apps and packages", height
  max(44, 3.4×pt), search icon on the left (accent when focused), clear
  (×) button on the right that fades in with the first character. 2 px
  accent ring only for keyboard focus (Tab/shortcut), 1 px accent on click
  focus.
- Typing: `queryDirty=true`, `frozenOrder=null`; empty text → submit now,
  else restart a **220 ms debounce** → `submitSearch()`.
- Enter → `submitSearch()` immediately. Down → focus list and select
  row 0 (without opening it).
- `submitSearch()`: stop debounce; only on Search; `queryDirty=false`;
  clear sort column; `reload()`.
- Empty prompt (centered, 10% above center, max 420 wide): 34 px muted
  `search` icon, "Find apps and packages" (DemiBold), then
  `searchHint()`:
  - before `source_catalog` arrives: saved `searchHint` setting, or
    "Searches your sources as you type."
  - no enabled available source with `search` capability:
    "No enabled source can search. Turn one on in Sources."
  - else "Searches {Name} as you type." / "Searches N sources as you type."
  - Saved to settings `searchHint` whenever the catalog changes.
- Results ranking (only when no column sort): see §4.5.
- When the query is non-empty and no sort column: rows are grouped by
  `same_app_group` (§4.6), Flatpak scopes merged (§4.7).

### 2.2 Installed
- Filter row (grid: 3 columns, 2 when compact):
  - Text field "Filter installed packages" (filter icon, clear button).
    Client-side only; matches lower-cased
    `name + " " + display_name + " " + summary + " " + source`. A row of a
    `same_app_group` also shows if any member of its group matches.
    Debounced 150 ms when > 400 rows, else live. Enter/Down → focus list,
    select row 0.
  - Checkbox **Duplicate installs** (session only): keeps package rows with
    non-empty `same_app_from`. Visible when any package rows exist, it is
    checked, or busy.
  - **Manage N AppImages** (`install` icon, tooltip
    "Manage N AppImages with PkgDeck"): visible when ≥ 2 rows with
    `source=="appimage"` and `canAdopt`. Enabled when `canAct` and not
    retaining. Click → `propose("adopt-all")` (§5.1).
- Rows always grouped by `same_app_group` (§4.6); Flatpak scopes merged.
- Backend query is always the full installed set (empty query).

### 2.3 Updates
- Each package row has a checkbox (TickBox, 28 px column). All rows start
  checked; the GUI stores **unchecked identities** so new streamed rows
  start checked. Cleared on reload and page switch.
- Actions (below the list), shown when ≥ 1 checked / ≥ 1 unchecked:
  - Primary **Update all** (no unchecked) / **Update selected (N)**
    ("Update" when compact). `updates` icon. Disabled while
    `busy && !writing`. → `upgradeUpdates()` (§5.1). Also Ctrl+Shift+U.
  - **Select none** (`cancel` icon) → uncheck all.
  - **Select all** (`installed` icon) → clear unchecked set.
  - Under 360 px page width those two show icons only (tooltip = name).
- Row action = Update (§4.8). Version column shows candidate with
  "from {installed}" below.
- Failure banner adds ". Update all will retry the check."

### 2.4 Clean
- Only `kind=="cleanup"` rows. Version column header "TYPE", values:
  `orphan_dependencies`→"Dependencies", `duplicate_copy`→"Extra copy",
  else "Cache". Row icon `remove`.
- Row action "clean" (Run cleanup …). Clicking a row opens the
  non-package details panel showing `details.cleanup.summary` +
  blank line + `details.cleanup.preview`.
- Primary **Clean all** (`remove` icon) when > 1 cleanup rows; enabled when
  `!busy || writing`. Click: `reviewOn("")`,
  `controller.propose("clean-all", -1)`.
- Extra-copy cleanup (`cleanup_kind == "duplicate_copy"`) is just a cleanup
  row like the others (an AppImage copy PkgDeck already manages elsewhere).

### 2.5 Sources
- Rows are `kind=="source"` (from `rows`, same shape as `source_catalog`).
  Unavailable sources hidden unless "Show unavailable (N)" toggled (button in
  the results heading; label "Hide unavailable" when shown).
- Default order: enabled sources first, then name, then source.
- Columns: "SOURCE" (name fills), "STATUS" (only when unavailable shown;
  "Available"/"Unavailable"), no summary column.
- Per available row: checkbox "Enabled"/"Disabled"
  (`checked = source in checkedSources()`), disabled while writing or when it
  is the last enabled one. Toggle → `setManagerEnabled(source, checked)`:
  persists `sourceList` (empty string = all), clears `source`, clears every
  page filter, reloads.
- Selecting a source row opens the details panel: text
  `details.availability + "\nLast successful check: " + when`, where `when`
  = locale string of `report_state.last_success[source]` or
  "No successful check yet".
- Actions: **Repositories** (`sources` icon) when any of flatpak, fwupd,
  apt, dnf, zypper is available; disabled while busy; opens §5.5.
  **Refresh sources** (primary, `refresh`) when a source row is selected;
  enabled when (`!busy || writing`) and that source is available →
  `propose("refresh")` (`controller.propose("refresh", rawIndex)`).
- `load` for Sources passes an empty sources CSV.

### 2.6 Settings (scrolling, cards max 760 wide, spacing 14)
Labels sit beside controls (control width 240); under 520 px they stack.
1. **APPEARANCE**
   - Theme combo: System / Dark / Light → setting `appearance` 0/1/2.
   - Switch **Animations** = `!reduceMotion`.
2. **UPDATE CHECKS**
   - Switch **Background checks** = `backgroundMode`. Turning it off while
     `autostart` is on first calls `controller.set_autostart(false)`; if that
     returns false the switch stays on. Off also sets `autostart=false`.
   - **Check every** slider, snapping steps (minutes →label): 15 "15
     minutes", 30 "30 minutes", 60 "1 hour", 180 "3 hours", 360 "6 hours",
     720 "12 hours", 1440 "1 day", 2880 "2 days", 4320 "3 days", 10080
     "1 week". Shows the label top-right. Starts at the step nearest the
     saved `checkInterval`. Disabled when background checks are off.
   - Switch **Install updates automatically** (`autoUpdate`), disabled when
     background checks off.
   - Switch **Allow updates that remove packages** (`allowRemovals`), info
     icon tooltip "Like an old kernel replaced by a new one".
   - Switch **Allow automatic updates without a password**: checked =
     `systemApproval != ""`; tooltip "Changes you start still ask". Click →
     `controller.allow_system_updates(checked)`; the switch then follows the
     saved value (it changes when `system_approval` changes after the
     password prompt). Below it, in danger color, `controller.approval_error`
     when non-empty (indented 28).
   - Switch **Start in background at login** (only Linux/macOS), enabled
     when background checks on AND tray available. Click →
     `controller.set_autostart(checked)`; on true save `autostart`, on false
     revert.
   - Separator, then: "Last check: {short datetime}, N update(s) found" or
     "Last check: never" (from `backgroundState`, §6.6). Warning icon
     (tooltip) when notifications won't work:
     - no tray: "No system tray, so no notifications" (macOS "No menu bar
       icon, so no notifications")
     - tray can't notify: "This system tray can't show notifications" (macOS
       "Notifications are off in System Settings")
     - macOS not authorized: "Notifications show Script Editor's icon until
       you allow PkgDeck"
     Buttons: **Notification settings** (macOS, permission needed) and
     **Test notification** (`bell`; enabled with background checks on and
     notifications available) → notify "PkgDeck test" / "Desktop
     notifications are working." (macOS: request permission first if not
     authorized).
3. **ABOUT**: 36 px logo, "PkgDeck {controller.version}" (DemiBold 1.15×),
   in rail mode the "Made with ♥ by astro" line, **GitHub** button (GitHub
   mark icon, dark/light variant) → open `https://github.com/astrovm/PkgDeck`.
4. **KEYBOARD SHORTCUTS**: two-column list (one under 620 px), keycaps in
   monospace 0.88× on `canvas` with `line` border, Mac symbols on macOS
   (§7.3). Entries: Search Ctrl+1, Installed Ctrl+2, Updates Ctrl+3, Clean
   Ctrl+4, Sources Ctrl+5, Search or filter Ctrl+F, Settings Ctrl+,,
   Activity Ctrl+J, Focus results Ctrl+L, Select result ↑ / ↓, "Install,
   remove or update the selected row" Enter, Update selected Ctrl+Shift+U,
   Install Ctrl+I, Remove Ctrl+D, Update Ctrl+U, "Refresh the source's
   package lists" Ctrl+M (macOS Ctrl+Shift+R), Reload the page Ctrl+R,
   Apply a confirmation Ctrl+Enter, "Clear search or close details" Esc,
   Quit Ctrl+Q.

Settings card style: `surface`, radius 12, 1 px `line`, padding 16, title
muted DemiBold 0.85× ALL CAPS letter-spacing 0.6.

### 2.7 Activity drawer (over any page)
- Scrim over the content: black at 0.45 (dark) / 0.22 (light), fades 120 ms,
  click closes. Drawer slides in from the right (x animation 120 ms),
  width `min(480, windowWidth - (windowWidth < 520 ? 0 : 48))`, full height,
  `surface`, margins 20.
- Header: "Activity" (bold, 1.25×) + close × (tooltip "Close (Esc)").
  Esc closes. Ctrl+J toggles. Opening always calls
  `controller.refresh_activity()`.
- **Cancel queued** button (when any entry is `queued`) →
  `controller.cancel_queued()`.
- List of entries, **newest first** (reverse of `activity`), spacing 8.
  Card per entry (radius 12, 3 px tone bar on the left):
  - Title: `target(operations[0])` (DemiBold, elided), result pill, time.
  - Result text: no outcomes → state map {queued "Queued", running
    "In progress", finished "Completed", failed "Failed", cancelled
    "Cancelled", interrupted "Interrupted"}; one outcome → Failed /
    Cancelled / Completed; batch → "All N completed" or "a completed, b
    failed, c cancelled" (omit zeros).
  - Tone: failed (state or any outcome) → danger; cancelled/interrupted →
    muted; queued → muted; running/authorizing → accent; else success.
    Pill = tone text on tone at 14%.
  - Time: `"Automatic · "` prefix when `frontend == "auto"`, then time only
    if today, else short date+time.
  - Folded batch: "and N more" (small, muted).
  - Running/authorizing entry: ActionProgress (§6.2). If
    `progress.activity_id == entry.id` use live progress fields and allow
    Cancel when `writing` → `controller.cancel()`; else label = target,
    indeterminate.
  - Expandable when > 1 operation, or log text, or `finished_at`. Click
    header / Enter / Space toggles (expanded ids survive refreshes).
    Expanded: remaining operations (one per line), "Finished {when} · took
    {duration}" (duration: "N s", "M min S s", "H h M min"), log box (mono,
    small, max 220 high, selectable) from `entry.log || entry.output`
    (array joined by newlines). Chevron rotates 180°.
- `target(op)`: kind = first key of the operation object; verb map
  install→Install, remove→Remove, upgrade→Update, upgrade_all→"Update all",
  refresh→Refresh, clean→Clean, else "Change". Name = `id.name || id.key`;
  where = [source display name of `id.backend`, scope ("System" / "User
  {uid}" / env path)] joined ", " in parentheses. (`labels[i]` from the
  controller is the same in people's words and may be used instead.)
- Empty: 34 px `activity` icon, "Nothing has run yet", "Installs, removals
  and updates you run will appear here."

---

## 3. Details and app pages

### 3.1 Details panel (non-package rows: sources, failures, cleanup)
- Shown under the list when the selected row's `kind != "package"` on a list
  page. Card `surface`, radius 12. Header: 44 px icon slot (accent tint
  bg, DeckIcon = source id for sources, `warning` for failures, else
  `package`), title = display_name || name (1.25×, DemiBold), source
  subtitle, the row's action button, close × ("Close details") → deselect
  and focus list.
- Body: `detailText()`:
  - `details.cleanup` → `summary + "\n\n" + preview`
  - `details.failure` → `failure.error + "\n" + hint`
  - `details.source` → availability + "\nLast successful check: …"
- Height: between `detailsMinimumHeight` and 48% (32% compact) of window,
  animated 200 ms.

### 3.2 App page (packages, and opened files/links)
Shown when `appPageOpen`:
- `opened != ""` (a file or link was opened): covers the whole content area
  (list hidden behind a `canvas` rectangle), has a **Back** button.
- or a package row was clicked (`pageIdentity == identity(selected)` on a list
  page): **embedded below the list** ("list + resizable details pane"), no
  Back button but a close ×.
Opening slides in from x+32 with fade (120 ms); closing an opened page slides
the list in from x-24 and fades from 0.4.

Header (left→right):
- Back (opened pages only, 38 px, `back` icon) → `closePage()`.
- Icon: 72 px (opened page) / 44 px (embedded). App icon image (`icon` path
  → `file://` URL, or https) else accent-tinted square with `package`. Source
  badge (20 px circle, `surface`, `line` border, source DeckIcon) on the
  bottom-right corner.
- Title: display_name || name (titleScale×1.35 on pages, 1.25 compact;
  wraps 2 lines in compact). Under it: source display name, version
  (monospace: installed || candidate), and in compact the scope selector.
- Flatpak scope selector (when the row has ≥ 2 `flatpakVariants`): segmented
  buttons "System" / "User"; current = variant whose identity equals the
  row. Click → `selectFlatpakInstallation(i)` (§4.7). Disabled when the
  action is disabled.
- **Launch** (`launch` icon): shown when an installed AppImage is open
  (`pageLaunchable && pageLaunch && !pageLaunch.error`) or the opened file is
  an installed AppImage. Primary (filled) unless an update waits.
  Click → `controller.launch_app(rawIndex)` (row) or
  `controller.launch_opened()` (opened); non-empty return = error text shown
  in danger color.
- Secondary **Manage** (`install`, accent) for AppImages that `canAdopt` when
  the main action isn't adopt → `runAdopt(currentIndex)`. Then note under
  header: "Manage moves it into PkgDeck, which then keeps it updated with
  its menu entry and icon."
- Main action (`actionText`): opened → `opened.action` ("Install"/"Manage"/""
  ; hidden when launchable), else Install/Remove/Update/Clean/"Manage with
  Homebrew" from `rowActionName`. Icon: updates / remove / install. Tone:
  opened success; upgrade/adopt accent; install success; remove danger.
  Remove is icon-only (tooltip "Remove"). Primary fill on pages unless
  danger or Launch is primary. Disabled when `!canAct`, retaining rows, or a
  page review is showing. Click → `runPageAction()`.
- Close × (embedded page / panel).

Action button style ("DetailsAction"): tone at 13% fill (20% hover, 26%
down), 40% tone border, tone text DemiBold; primary = solid tone fill,
`surface`-colored text. Compact windows: icon only + tooltip.

Under the header:
- Launch error (danger) when the launch editor isn't shown.
- FUSE note (warning) when `appFile.without_fuse`: "Starts unpacked: this
  computer doesn't have FUSE 2 (libfuse2), which this AppImage needs to
  start the usual way."

Scrolling body:
1. **On-page review card** (§5.2) when `pageReview` is set.
2. Skeleton (4 pulsing bars at 92/100/78/46% width, 10 px high, pulse
   0.45↔1 over 800 ms) while details for this row haven't arrived
   (`loading`) or `details.more == true`.
3. Loaded body (fades in 120 ms):
   - Screenshot strip (horizontal): thumbnails 360×225 on opened pages,
     230×130 embedded, 128×72 compact. Placeholder pulse while loading.
     Click → screenshot dialog. Failed images are dropped from the strip
     (per selected identity).
   - Summary (page only, titleScale DemiBold) when it differs from the
     description.
   - Description (selectable plain text, ink at 86%). On a page, a
     description equal to the row's summary is hidden.
   - Facts grid (label muted small | value selectable): Updates
     (`updateState`, AppImage only), Publisher, License, File (appFile.path
     or details.location), Size (`appFile.bytes`, units bytes/KB/MB/GB base
     1000, 1 decimal under 10), Updated (`appFile.modified` short date),
     then Homepage link (scheme stripped, accent, `external` icon; opens
     URL only if http(s)).
   - **Show in folder** when `appFile.folder` → open
     `file://` + each path segment percent-encoded.
   - Dependencies toggle "Dependencies (N)" (chevron rotates 90°), collapsed
     by default and reset when the selection changes; expanded = wrapped
     chips.
   - Launch settings editor (page, `launchSettings.editable == true`):
     "Command line arguments" mono field; "Environment variables" rows of
     NAME = value fields with icon-only remove; **Add variable**; **Save**
     (enabled when edited) → `controller.save_app_launch_settings(rawIndex,
     arguments, envJson)` where `envJson = [{"name","value"},…]`. Empty
     return → bump `launchRevision` (re-read settings); else show error.
     Fields reset whenever `launchSettings` changes.
   - Update source editor (page, `updateSource.editable == true`): "Updates
     from GitHub", field placeholder "owner/name", **Save** (enabled when
     edited; Enter also saves) →
     `controller.save_app_update_source(rawIndex, github)`; empty return →
     bump `launchRevision`, else show error (danger).

AppImage page data (only when the open row is an installed AppImage; re-read
on open and after each successful save):
- `controller.app_launch_settings(rawIndex)` → `{"arguments": str,
  "environment": [{"name","value"}], "editable": bool}` or `{"error": str}`
  or `{}`.
- `controller.app_update_source(rawIndex)` → `{"github": str|null,
  "builtin": bool, "editable": bool}` or `{"error"}` or `{}`.
- `controller.app_file(rawIndex)` → `{"path", "bytes", "modified" (epoch
  s), "managed": bool, "without_fuse": bool, "folder": str|null}` or `{}`.
- `updateState`: update available → "Update {candidate} available" (or
  "Update available"); github set → "From GitHub, {github}"; builtin →
  "Updates itself"; editable → "No update source yet. Add its GitHub project
  below."; else "".

Header data for an opened page comes from `opened` (§6.1); for a row page
from the row plus `details` when `details.package` identity == selected
identity (`detailMatchesSelection`).

### 3.3 Embedded page height (list + resizable pane)
- Height = saved `detailsHeight` (> 0) clamped to `[min, max]`, else
  automatic: `min(windowHeight×0.5 (0.45 compact), idealHeight, max)`.
  Animated 200 ms except while dragging.
- `max = budget − listHeightWithDetails`; `min = min(120 (72 compact),
  idealHeight, budget×0.4 (0.34 compact), budget − listChrome − oneRow)`.
  Budget = window content height minus every other visible page child.
  The list keeps at least one full row (2–3 rows by default) above the pane.
- Resize handle on the pane's top edge (14 px tall, 8 px above it): grip
  36×4, accent on hover/drag/focus. Drag up = taller; saves `detailsHeight`.
  Double-click or Home → 0 (automatic). Up/Down ±24 when focused.
- Opening details shrinks the list; keep the selected row scrolled into view.

---

## 4. Results list

### 4.1 Card
- `surface`, radius 12, border `line` (accent when the list has focus).
- Visible when `currentView == resultView` and not Settings, and on Search
  only if there are rows, failures, a running load, or a query.
- Height: fills the page while the first load is empty or rows overflow;
  otherwise content height (min 130; 150 when empty and idle).
- Heading row (margins 14; shown when rows exist, busy, writing, or Sources
  has unavailable rows):
  - `resultsHeading()` (muted, 0.9×): writing → "Applying changes…"; busy →
    Search "Searching…", else "Refreshing…" (rows shown) / "Loading…";
    Search with `report_state.matches > count` → "Best {count} of {matches
    localized} packages. Type more to narrow."; else "N package(s)" (noun
    "source"/"cleanup task"/"update" on Sources/Clean/Updates).
  - "Waiting for A, B" (muted, ≤ 60% width): `pending_sources` mapped to
    display names, excluding sources whose availability is unavailable or
    platform; shown only after the read has been running 800 ms.
  - Spinner (open-arc `spinner` icon, accent, 1.1 s rotation) while
    (`busy && !writing`) or `refreshing`, if rows exist and motion is on.
  - "Show unavailable (N)" / "Hide unavailable" (Sources).
  - **Cancel** (`cancel`) while `busy && !writing` → `controller.cancel()`.
  - Reload (flat `refresh` icon, tooltip "Reload (Ctrl+R)") when
    `!busy || writing` → `reload(true)`.
- 1 px divider, then column headers (hidden in compact): optional 28 px
  checkbox column (Updates), icon column (when page width ≥ 420),
  "NAME / SOURCE" ("SOURCE" on Sources) width `nameWidth`, "VERSION"
  ("TYPE" Clean, "STATUS" Sources) width `versionWidth`, "SUMMARY" fills
  (hidden in medium layout and on Sources). Caption size, DemiBold, muted,
  letter-spacing 0.6, sort arrow " ▲"/" ▼".
  - Click / Space / Enter cycles sort: new column → ascending; same →
    flips. Sort keys: name→`name`; version→`installed || candidate`;
    summary/status→`summary`; capabilities→joined capabilities. Lower-cased,
    ties broken by name then source (direction-aware). Saved as
    `sortColumn`/`sortAscending`.
  - 12 px drag handle on the right of NAME and VERSION resizes 80..600
    (saved `nameWidth`, `versionWidth`); Shift+Left/Right ±16 when the
    header has focus.

### 4.2 `reload(force = false, preserveSelection = false)`
1. Retain the current rows on screen (`retainingResults`) when reloading the
   same page with the same query, or refining a Search; retained rows hide
   only once the new rows cover at least as many rows or the load ends
   (`retainUntilDone` for the same query). While retaining, the list is
   disabled and Search narrows retained rows by substring.
2. Empty Search: don't query; clear old results.
3. `resultQuery = trimmed search text` (Search).
4. Clear list selection; unless `preserveSelection`, `selectedIdentity=null`;
   clear Updates unchecked set.
5. Call **`controller.load(view, query, sources, sudo, force)`**:
   - `view`: "Search" | "Installed" | "Updates" | "Clean" | "Sources"
   - `query`: search text on Search, else ""
   - `sources`: Sources → ""; else `checkedCsv()` = comma list of
     effective sources for this page, or "" when every known source.
   - `sudo`: true only with CLI `--auth sudo`
   - `force`: true for Reload/Retry/post-write; false for page switches and
     typing (cached rows < 60 s publish instantly; older ones publish and
     quietly refresh with `refreshing=true`).
6. If not busy right after, stop retaining.

Callers: page switch, `submitSearch`, Reload button, Ctrl+R (`force`),
"Search again"/"Load again"/"Check again", retry buttons, filter apply,
source enable/disable, clearing a page filter, startup, and after every
write (§6.5).

### 4.3 Row shapes (`rows` = JSON array)
- **package** (`kind:"package"`): `name, display_name, source, architecture,
  remote|null, reference|null, scope, scope_label, summary, installed
  (str|null), candidate (str|null), update ("unknown"|"current"|"available"),
  icon (path|https|null), same_app_from [source ids], same_app_group
  (str|null), adopt_with (str|null: "appimage", cask token…)`.
- **failure** (`kind:"failure"`): `name, source, display_name, summary,
  failure_kind` (packages only; one of unsupported, unavailable, cancelled,
  authorization, locked, partial, failed), `available:false`. Appended after
  packages. Never listed as rows (filtered out), they feed failure UI.
- **cleanup** (`kind:"cleanup"`): `name, display_name (title), source,
  summary, cleanup_key, cleanup_kind ("orphan_dependencies"|"package_cache"|
  "duplicate_copy"), preview`.
- **source** (`kind:"source"`): see `source_catalog` (§6.1).
Rows are packages + failures (Search/Installed/Updates), cleanups +
failures (Clean), or sources (Sources). Raw index = position in this array.

### 4.4 Visible rows (`viewItems`) pipeline
1. Drop `failure` rows. Search also drops "fabricated" rows (package with
   neither installed nor candidate). Clean keeps only cleanup rows. Sources
   hides unavailable unless toggled.
2. Except Sources: keep rows whose `source` is in `effectiveSources()`.
3. Installed text filter + Duplicate installs (§2.2).
4. Sort: column sort if set; else Sources default order; else Search
   relevance (§4.5) and frozen order.
5. Search/Installed: merge Flatpak scopes (§4.7).
6. Installed always, Search when query non-empty and no sort: group by
   `same_app_group` (§4.6).
`originalIndex(visible)` = first raw index whose identity equals the visible
row's identity (−1 if none). Every controller call with an index uses it.

### 4.5 Search relevance
- Query q = trimmed lower-case text. If retained/old rows were for another
  query, first narrow them to rows whose `name display_name summary` contains q.
- Score: 0 exact name or display name, or Flatpak id's last dotted segment ==
  q; 1 name prefix; 2 name substring; 3 summary prefix; 4 summary substring;
  5 otherwise; +6 for fabricated rows. Ties: name, then source.
- **Frozen order**: while a Search still streams (`busy && !writing`) and the
  pointer is over the list or it has focus, freeze each row's current
  position; late rows go below, best first. Unfreeze when leaving Search, or
  when not busy and not engaged; typing clears it.

### 4.6 Same-app groups
- Rows sharing `same_app_group` (≥ 2 members) are emitted together at the
  first member's place. The first carries a 38 px group header inside the
  row: `selection` background, 3 px accent bar, bold title (first member's
  `display_name`, else the shortest name), right side source display names
  joined " · " in accent, small.

### 4.7 Flatpak scope merge
- Group key `[name, architecture, remote||"", reference||""]`. A group with
  both a system and a non-system copy becomes **one row**: the remembered
  choice (`flatpakScopeChoices[key] == identity`), else an installed copy,
  else system. The row carries `flatpakVariants` (system first) and
  `flatpakGroup`. Source line says "System and user".
- `selectFlatpakInstallation(i)` (needs `canAct`, not retaining): save
  `flatpakScopeChoices[group] = identity(variants[i])` (JSON object string,
  saved immediately), then `choose(currentIndex)` again (re-selects the
  new copy → `controller.select`).
- If a selected copy disappears after reload, reselect the remaining copy of
  the same group (keeping the page open if it was).

### 4.8 Row layout
- Height: normal `max(56, 5×pt)`; compact cards `max(94, 8.5×pt)`
  (sources `max(68, 5.5×pt)`); +38 with a group header.
- Background: hover `hoverTint`, selected `selection`, radius 9, inset;
  1 px focus ring (accent) only while navigating by keyboard. Bottom
  divider `line` at 60%. Recently changed rows flash `success` at 18% for
  1 s (§6.5). Row progress line (§6.3) along the bottom.
- Cells: Updates checkbox; icon slot (32 px, 40 compact; hidden < 420 px
  page width): app icon (async; fallback = source DeckIcon on hoverTint, or
  `remove` for cleanup) with 16 px source badge when the icon loaded;
  name column: bold name (display_name || name; source display name for
  source rows), muted small `sourceLine` (§4.9); compact adds version (mono,
  small) and summary lines. Wide: version column (mono small; update rows
  show candidate, accent, + "from {installed}" caption; failures danger;
  updates accent; else muted; middle-elided) and summary (muted, elided).
- Trailing buttons (38 px flat icons):
  - Container pull (`updates`, accent): docker/podman installed rows with
    `reference`, when not running. →
    `reviewOn(id); markActiveRows([id]); controller.propose("upgrade", raw)`.
    Tooltip "Pull {name} from {sourceLine}".
  - Row action (hidden for the row whose app page is open):
    `rowActionName(row)`:
    - cleanup → "clean"; non-package → none
    - `macos-apps`: canAdopt → "adopt", canRemove → "remove"
    - never-installs source: update available → "upgrade", canRemove →
      "remove"
    - Updates page → "upgrade"; installed → "remove"; else "install"
    Icon: running → `cancel`; upgrade → `updates`; install/adopt →
    `install`; else `remove`. Color: disabled muted; running muted;
    upgrade/adopt accent; install success; remove/clean danger. Tooltip:
    "Install|Remove|Update|Run cleanup|Manage {name}" + (cleanup: "" ; adopt:
    " with Homebrew"; else " from {sourceLine}"); running → "Cancel
    {controller.status || 'the change'}". Enabled when running, or
    (`canAct` and not retaining). Click: running → `controller.cancel()`;
    else `runRowAction(visibleIndex)`.
- Accessible name: "Update available. " / "Installed. " / "Not installed. "
  + name + ", " + sourceLine + ", " + summary.

### 4.9 Display helpers
- `sourceLine(row)` = join ", " of non-empty: package-name note (package
  name when it differs from display_name ignoring case/punctuation; not for
  flatpak, macos-apps, mas, appimage), source display name, remote, scope
  label (merged Flatpak "System and user"; flatpak/docker/podman "System"
  or "User"), Flatpak branch when ≠ "stable" (last `/` part of reference),
  "not managed" (AppImage installed elsewhere: `adopt_with=="appimage"`).
- `versionText(row)`: cleanup type; failure "Failed"; source
  Available/Unavailable; update available → "old → new" (or the one known,
  else "Unknown"); installed → installed or "Unknown"; else candidate or
  "Unknown".
- `isInstalled(row)` = `installed` not null/undefined (empty string counts).
- `canRemove(row)` = package, installed, and (source not macos-apps and not
  never-installs, or the catalog lists `remove` capability for it).
- `canAdopt(row)`: package with `adopt_with`; AppImage → `adopt_with ==
  "appimage"`; `macos-apps` → on macOS and Homebrew Casks available.
- Never-installs sources: fwupd, mas, macos-updates, conda, system-image,
  aur, toolbox, distrobox, oh-my-zsh, codex, claude, grok, opencode, cursor,
  copilot, kiro, antigravity, amp, droid, solana, anchor, foundry.
- Source ids → display names (fallback: the id): apt APT, dnf DNF, pacman
  Pacman, zypper Zypper, snap Snap, homebrew Homebrew, homebrew-cask
  "Homebrew Casks", macos-apps "macOS Applications", mas "Mac App Store",
  aur AUR, apk apk, xbps XBPS, system-image "System image", macports
  MacPorts, macos-updates "macOS Updates", rustup rustup, nix Nix, go Go,
  dotnet ".NET tools", appimage AppImage, flatpak Flatpak, docker "Docker
  images", podman "Podman images", toolbox "Toolbx containers", distrobox
  "Distrobox containers", cargo Cargo, npm npm, pnpm pnpm, bun Bun, pip pip,
  pipx pipx, uv uv, mise mise, pixi pixi, conda Conda, composer Composer,
  gem RubyGems, oh-my-zsh "Oh My Zsh", fwupd Firmware, codex "Codex
  (standalone)", claude "Claude Code (standalone)", grok "Grok
  (standalone)", opencode "OpenCode (standalone)", cursor "Cursor CLI
  (standalone)", copilot "GitHub Copilot CLI (standalone)", kiro "Kiro CLI
  (standalone)", antigravity "Antigravity CLI (standalone)", amp "Amp
  (standalone)", droid "Factory Droid (standalone)", solana "Solana CLI
  (Agave)", anchor "Anchor (AVM)", foundry Foundry. Known ids list is in
  this order; catalog ids not in it are appended.
- Source categories (picker): System = apt dnf pacman aur zypper apk xbps
  macports macos-updates system-image fwupd; Applications = snap homebrew
  homebrew-cask macos-apps mas appimage flatpak; Containers = docker podman;
  else "Developer tools".

### 4.10 Selection and opening
- `choose(visible, open = true)`: ignored while retaining or out of range.
  open → `pageIdentity = identity` (opens the app page for packages). Sets
  current index and `selectedIdentity`, remembers the Flatpak group. Calls
  **`controller.select(rawIndex)`** unless the same row is already
  selected.
- Click row → focus list, mouse navigation mode, `choose(i)`.
- Arrow/Page/Home/End → `choose(i, false)` (selection only).
- Hover a package row → after **150 ms** rest,
  **`controller.warm_details(rawIndex)`** (skip while retaining). Leaving
  cancels the timer.
- After rows change / load ends (`restoreSelection`, deferred): find
  `selectedIdentity` in visible rows and reselect it; if
  `details == "{}"` and not busy, call `controller.select(raw)` again.
  Else Flatpak fallback (§4.7). Else clear selection (keep identity only on
  Search when the raw rows still contain it).
- `closePage()`: opened → `controller.close_opened()`; else
  `pageIdentity = null`. Then focus the list.
- `selected` = visible row at current index (null while retaining).

### 4.11 Empty and failure states (inside the card)
- Loading with no rows: up to 8 placeholder rows (two bars + version +
  summary bars, `hoverTint`), pulsing 0.55↔1 over 700 ms.
- Otherwise centered: 34 px icon + DemiBold message `emptyStateMessage()`:
  1. writing on Search with no rows → "Search will resume shortly."
  2. busy or phase "loading" → nothing
  3. read failures and (no source answered, phase failed, or page not
     Updates/Clean) → `sourceFailureTitle()`
  4. failures and no rows → Updates: "{A and B | N other sources} is/are up
     to date"; Clean: "Nothing to clean in …"
  5. cancelled load, no rows → "Search stopped" / "Loading stopped"
  6. phase "unsupported" → "None of your enabled sources support this page."
  7. Search with dirty or empty query → nothing
  8. Installed with filters → "No packages match these filters."
  9. page filter and raw rows exist → "No results from selected sources."
  10. Updates "You're up to date"; Clean "Nothing to clean"; Installed
      "No installed packages."; Sources "No available sources."; Search
      "No matching packages."; else "No results to show."
  Icon: failures (and not good news) → `warning`/warning color; "…up to
  date" or "Nothing to clean…" → `installed`/success; Search → `search`;
  else `package`/muted.
- Hint line: with failures → (good news ? title + ". " : "") + "Retry to
  check for updates." (Updates) / "Retry to check again."; colored warning
  when good news. Without failures on empty Updates/Clean →
  `checkedAgo()` ("Checked just now" / "N minute(s) ago" / "N hour(s) ago"
  / short datetime), refreshed every 30 s.
- With failures: **Retry** (one failure → `retryFailedSource(source)`) or
  **Reload** (several → `reload(true)`), and **Details** (failures dialog).
- Cancelled: **Search again** / **Load again** → `reload(true)`.
- Otherwise (not Search, heading hidden): **Reload** / **Check again**
  (Updates/Clean) → `reload(true)`, and **Clear filters** when a page filter
  or Installed filter is set → clear text filter, Duplicate installs, and
  this page's source filter (reload only if a source filter was cleared).
  Icons only if both labels don't fit.

---

## 5. Dialogs, popups, banners and changes

### 5.1 Starting changes
- `reviewOn(identity)`: `reviewFor = (rowPageOpen && !opened && identity ==
  pageKey) ? pageKey : ""`. `pageKey` = "" (no page), "opened", or the open
  row's identity. Decides whether the review shows on the page or in the
  dialog.
- `markActiveRows(ids)`: rows that show progress/Cancel; cleared when
  `writing` turns false or on reject.
- `runRowAction(visible)` (row button, Enter, page main action):
  `reviewOn(id)`; if action and not retaining and `canAct`: mark row,
  `quickChange = true`, **`controller.propose(action, raw)`** with action ∈
  install | remove | upgrade | clean | adopt.
- `runAdopt(visible)`: `reviewOn(id)`; if `canAdopt` and allowed: mark row,
  `controller.propose("adopt", raw)` (no quickChange).
- `runPageAction()`: opened page → if `canAct`: `reviewFor = pageKey`,
  `quickChange = true`, **`controller.install_opened()`**; else
  `runRowAction(currentIndex)`.
- `propose(action)` (shortcuts, Manage all, Refresh sources):
  `reviewOn("")` for upgrade-all/adopt-all/refresh, else selected identity;
  unless retaining: mark rows (all Updates identities for upgrade-all; all
  adoptable AppImages for adopt-all; else the selected row), then
  `controller.propose(action, originalIndex(currentIndex))`.
  Shortcuts: Ctrl+I "install", Ctrl+D "remove" (only if `canRemove(selected)`),
  Ctrl+U "upgrade", Ctrl+M (macOS Ctrl+Shift+R) "refresh"; all enabled when
  `!busy || writing`.
- `upgradeUpdates()`: nothing unchecked → `propose("upgrade-all")`; else
  `reviewOn("")`, mark checked ids, **`controller.propose_checked(json)`**
  where json = array of the checked identities parsed back to arrays
  (`[[source,name,arch,remote,scope,reference], …]`).
- Clean all: `reviewOn("")`, `controller.propose("clean-all", -1)`.
- `canAct = !busy || writing || reading`.
- Undo (toast action): from the current notice, if `undo == true` and
  `undo_action` ∈ install/remove and `target` set: find the raw row with
  `target`'s identity, mark it, `controller.propose(undo_action, rawIndex)`.

### 5.2 Confirmation routing (on `confirmation` change)
When `confirmation` becomes non-empty:
1. `openingInput = false`. Parse `confirmation_data`.
2. If `quickChange` was set and `confirmation_data.review == false`:
   auto-confirm on the next frame (`controller.confirm(true)`) if
   `confirmation` is unchanged by then. Clear `quickChange` either way.
3. Else, if an app page is open, `reviewFor == pageKey`, and there is no
   `flatpak_ref_scope`: show the **on-page review** (`reviewOnPage = true`).
4. Else open the **Confirm changes** dialog.
When `confirmation` becomes empty: `reviewOnPage = false`, close the dialog.
If `pageKey` changes while an on-page review is shown for another page:
drop it (`activeRows = []`, `controller.confirm(false)`).

On-page review card (inside the page body; tint of accent, or danger when
the action word is "Remove", 10% fill, 35% border, radius 12):
- Title = `(data.action || "Apply") + "?"`; then `data.notes` lines (muted);
  then `data.changes` joined by newlines (selectable, small, muted).
- Buttons: apply (label = first word of `action`, e.g. Install/Remove/
  Update; `remove` icon + danger for Remove, else `install`) →
  `controller.confirm(true)`; **Cancel** → `activeRows = []`,
  `controller.confirm(false)`. Esc also cancels; Back is disabled while it
  shows.

### 5.3 Confirm changes dialog (modal)
- Width `min(window−32, 600)`, height fits content (min 230). Title
  "Confirm changes". Header close × = reject. Scrim black 0.5/0.28. Opens
  with fade + scale 0.96→1 (120 ms).
- Body:
  - Flatpak ref scope row when `flatpak_ref_scope` ("system"/"user"):
    "Install for" + combo User/System → on change
    **`controller.set_open_flatpak_scope(index == 1)`** (re-previews; a new
    `confirmation_data` arrives).
  - Summary box (`selection` bg, radius 10): optional 48 px icon
    (`data.icon`), first line of `summaryText` bold, remaining lines muted,
    where `summaryText = data.summary || data.body || confirmation`.
  - **Show details** / **Hide details** toggle (when `data.details`),
    collapsed each time it opens; box (`canvas`, `line` border) with
    `details` text.
- Footer right: Apply (primary; text = first word of `data.action` or
  "Apply"; danger fill with `#0e1015`/`#ffffff` text when "Remove") and
  **Cancel** (gets initial focus).
- Keys: Ctrl+Enter / Ctrl+Return apply; Alt+C cancels; Esc cancels; Alt+X
  applies where X = first letter of the apply word that isn't "c" (that
  letter is underlined).
- Accept → `controller.confirm(true)`. Reject → `activeRows = []`,
  `controller.confirm(false)`. Focus returns to the previous control.

`confirmation_data` shape (any key may be missing): `action` (title, ≤ 36
chars + "…"), `body`, `summary` (first line = title/subject, more lines =
notes), `details`, `flatpak_ref_scope` ("", "system", "user"), `icon`
(path|null), `review` (bool; false = only the asked app changes),
`notes` [str], `changes` [str]. `confirmation` itself is the plain-text
body; non-empty means a change awaits an answer.

### 5.4 Source checks dialog ("Details" on failures)
- Modal, width ≤ 620, title "Source checks", Close button.
- Per read failure: bold source name; **Turn off** (flat, when kind ≠
  partial and `canTurnOff`) → `setManagerEnabled(source, false)` and close;
  **Retry** (enabled when not busy, kind ≠ unsupported, and Search query
  isn't dirty) → `retryFailedSource(source)`; muted reason
  `failureSummary(source)` = failure row summary || report detail || (on
  Sources the catalog summary) || "This source could not be checked."
- **Copy diagnostics** → clipboard:
  `"View: {page}\nState: {phase}\n" + lines "{source} ({kind}): {reason}"`.
- `retryFailedSource(id)`: skip on Search with dirty query;
  **`controller.retry_source(view, query (Search only else ""), id)`**;
  close the dialog.
- Read failures = `report_state.failures` (or failure rows when absent,
  excluding `unsupported`), excluding kind `cancelled` and sources outside
  `effectiveSources()`; only for the page the rows belong to.
- `canTurnOff(id)` = source is enabled and more than one is enabled.

### 5.5 Repositories dialog (Sources → Repositories)
- Modal, width ≤ 850, height fits list (min 260, max 85%). Title
  "Repositories", Close button. On open: **`controller.load_repositories()`**.
- Top actions: **Add Flatpak repository** (when `features.flatpak_user ||
  flatpak_system`), **Edit APT sources** (when `features.apt_editor`) →
  `changeRepository({backend:"apt", name:"sources", scope:"system"},
  "open_editor")`, reload icon → `load_repositories()`. All disabled while
  busy. Spinner while busy.
- Card per repository (`canvas`, radius 10, two columns ≥ 620 px):
  - Checkbox: editable (fwupd always; flatpak when the matching
    `flatpak_system`/`flatpak_user` feature) → toggling calls
    `changeRepository(row, "set_enabled", {enabled})` and snaps back to the
    model value; read-only dim checkbox otherwise.
  - Title (`title || name`), muted "{source display name} · System|User",
    URL (middle-elided) unless already in the title.
  - Right: "Priority N" when set; for editable Flatpak: up (priority+1,
    disabled at 9999), down (−1, disabled at 0) → `set_priority`; remove
    (`remove` icon) → `"remove"`.
- `errors` lines (muted) and `controller.status` (muted) unless it is
  "Ready" or "Repositories loaded.".
- `changeRepository(row, action, extra)` →
  **`controller.change_repository(JSON)`** with JSON
  `{"backend","name","scope","action", …extra}`; action ∈ add (`url`),
  remove, set_enabled (`enabled`), set_priority (`priority`), open_editor.
  Except open_editor, the controller answers with a confirmation (action
  "Apply") shown in the Confirm changes dialog.
- **Add Flatpak repository** dialog: Name field (valid
  `^[A-Za-z0-9._][A-Za-z0-9._-]*$`, error "Use letters, numbers, dots,
  dashes, or underscores. Do not start with a dash."), URL field
  placeholder "https://…/repository.flatpakrepo" (valid
  `^https://\S+\.flatpakrepo$`, error "Enter an HTTPS .flatpakrepo URL."),
  scope combo with "User"/"System" per features. OK enabled only when both
  valid → `changeRepository({backend:"flatpak", name, scope:"system"|"user"},
  "add", {url})`. Fields clear on open; name focused.
- `repositories` JSON: `{"repositories":[{"backend","name","title","url",
  "scope","enabled","priority" (int|null),"source_name","where"}],
  "errors":[str],"features":{"flatpak_user","flatpak_system","apt_editor"}}`.

### 5.6 Install from a file or link dialog
- Modal, width ≤ 480, title "Install from a file or link", Cancel button.
- Field placeholder "Paste an HTTPS link" (cleared on open, focused).
- **Choose file…** → close and open a native file picker titled "Open
  installation file", filter "Packages and sources (patterns)" (§5.10);
  accepted file → `openExternalInput(fileUrl)`.
- **Preview link** (primary, enabled when text matches
  `^(?:flatpak\+)?https://\S+$`; Enter also) → close,
  `openExternalInput(link)`.

### 5.7 Source picker popup (page filter)
- Anchored under the Filter button, right-aligned, width ≤ 340, height
  ≤ 85% window; Esc / click outside closes. Fades + scales from 0.95.
  On open: `controller.check_sources()`; draft = effective sources that are
  available and support this page; search cleared and focused.
- "Checking sources…" until `source_catalog` arrives (then the draft is
  filled if empty).
- "Find a source" field with clear button: filters by display name + id.
- Checklist grouped with ALL-CAPS caption headers: System, Applications,
  Developer tools, Containers, then (when **Show unavailable** toggled)
  "Unavailable". Each row: TickBox + name; disabled when unusable, not
  enabled in Sources, or it is the last checked. Unusable rows add a muted
  reason: "Not supported in this view" or the catalog summary.
  Page capability: Search→`search`, Installed→`installed`,
  Updates→`upgrade`, Clean→`clean`.
- **Reset** → draft = all enabled usable sources. **Apply** (primary,
  needs ≥ 1) → store the page filter (removed when it equals all enabled),
  close, reload.
- `effectiveSources(view)` = enabled sources ∩ page filter (if any).
  `sourceSummary()`: one source → its name; no filter → "Available sources"
  or "N enabled sources"; filter → "N sources".
- Page filters are session-only and reset when Sources enablement changes.

### 5.8 Notice banner and toast (`notice`)
`notice` JSON: `kind` ("success"|"info"|"error"), `title`, optional
`detail`, `output` (raw tool output, errors only), `retry` (bool),
`operation` ("install"|"remove"|"update"|"update_all"|"refresh"|"clean"|
"batch"|""), `source`, `source_name`, `package` (display name), `target`
(row-shaped id), `undo` (bool), `undo_action` ("install"|"remove"|"").
`"{}"` = none.
- success/info → **toast** (§5.11): text = title, action "Undo" when undo
  applies, tone = kind. When the toast hides (timeout, close, or action),
  `controller.dismiss_notice()` if a notice is still set.
- error (and anything not success/info), not on Settings → **banner**
  under the progress line: tone tint 12%/8% fill, 35% border, `warning`
  icon, bold title, muted detail. Buttons: **Show details**/**Hide
  details** (when `output`) revealing a monospace selectable box (≤ 200
  high) and **Copy**; **Retry** (when `retry`, enabled when not busy) →
  `controller.retry_change()`; dismiss × → `controller.dismiss_notice()`.
  Output collapses when the notice changes. Fades 120 ms.

### 5.9 Source-failure banner
- When read failures exist and rows exist on a list page: warning tint box,
  `warning` icon, `sourceFailureTitle()` ("Some apps couldn't be read" if
  all partial; "Couldn't check {Name}"; "Couldn't check N sources") (+
  Updates suffix). Buttons: **Retry** → `reload(true)` (disabled when busy
  or dirty Search), **Turn off {Name}** (single non-partial failure,
  `canTurnOff`) → `setManagerEnabled(src,false)`, **Details** → §5.4.

### 5.10 Opening files and links
- `openExternalInput(input)`: `openingInput = true`;
  **`controller.open_input(input)`**; if then `!busy` and no confirmation,
  `openingInput = false`. It also clears when a confirmation arrives or
  busy ends (checked a frame later). Inputs: `file://…`, absolute path,
  `https://…`, `flatpak+https://…`.
- Result: `opened` becomes non-empty → full app page (§3.2), or a
  confirmation (repository files) → dialog.
- `supportedFilePatterns()`: no catalog yet → Linux `*.AppImage` only, else
  none. Else by available managers: appimage `*.AppImage`; apt `*.deb
  *.sources *.list`; dnf or zypper `*.rpm *.repo`; pacman `*.pkg.tar.zst
  .xz .gz .bz2 .lz4`; flatpak `*.flatpak *.flatpakref *.flatpakrepo`; snap
  `*.snap`; zypper `*.ymp`.

### 5.11 Toasts
- Bottom-center card (≤ 460 wide, max text width 400, 3 lines), `surface`,
  radius 12, `strongLine` border, soft shadow (black 0.35/0.1, offset 3).
  Tone icon (success `installed`, danger/warning `warning`, else `info`),
  text, optional action button (accent DemiBold), close ×. Slides up 12 px
  and fades in (120 ms); fades out (200 ms). Hides after 5 s, never while
  hovered (timer restarts on hover out).
- Bottom margin 24, or above the page action buttons when they would be
  covered.
- Restart toast: "PkgDeck was updated. Restart it to use the new
  version.", action "Restart", tone success, stays 24 h, stacks above the
  change toast. Action → `restartPkgDeck()`.

### 5.12 Screenshot dialog
- Modal, `min(window−32, 1040) × min(window−32, 720)`, title = caption or
  "Screenshot", Close. Image fit (decoded at shown size × DPR), spinner
  while loading, "Screenshot unavailable" on error. Focus returns on close.

### 5.13 Focus restore
Every dialog/popup remembers the focused control on open and restores it on
close (falls back to the results list).

---

## 6. Controller properties, polling and timers

### 6.1 Properties read (JSON text unless bool)
| Property | Shape | Use |
| --- | --- | --- |
| `rows` | array of rows §4.3 | the list |
| `details` | `"{}"`, or one of: `{"package": row, "description", "homepage", "publisher", "license", "dependencies":[str], "screenshots":[{"url","caption"}]\|null, "more": bool}`; preview `{"package","description","more":true}`; error `{"package","description":"Details couldn't be loaded. …"}`; `{"cleanup":{"id":{"backend","key"},"kind","title","summary","preview"}}`; `{"failure":{"backend","error"},"hint"}`; `{"source","availability","capabilities"}` | details/app page; used only when `details.package` identity == selected identity |
| `status` | plain text | row Cancel tooltip, repositories dialog |
| `notice` | §5.8 | banner/toast |
| `progress` | `{"activity_id":int\|null,"label","done","total","transferred","transfer_total":int\|null,"fraction":0..1\|null,"targets":[id rows],"sources":[ids],"action","current":id row\|null,"finished":[id rows],"current_source":str\|null}`; `"{}"` idle | §6.2-6.3 |
| `repositories` | §5.5 | dialog |
| `source_catalog` | `[{"kind":"source","name","source","summary","available":bool,"availability_kind":"available"\|"restricted"\|"unavailable"\|failure kinds,"check_failed":bool,"capabilities":["search","details","installed","install","remove","refresh","upgrade","clean",…]}]` | availability, capabilities, picker, file patterns, search hint. Missing id → `{availability_kind: catalog empty ? "checking" : "platform", summary: "Checking availability…" / "Unsupported on this platform", capabilities: []}` |
| `report_state` | `{"phase":"loading"\|"complete"\|"partial"\|"failed"\|"unsupported"\|"stale","failures":[{"source","kind","detail"?}],"successful_sources":[ids],"last_success":{id: epoch},"checked_at":epoch?,"matches":int?}` | headings, empty states, failures |
| `pending_sources` | `[source ids]` | "Waiting for …" |
| `manifest_preview` | `{"schema_version","packages":[{"package":{"backend","name","architecture","scope",…},"status","reason","proposed_changes":[{"kind","detail"}]}]}` | not used by the QML |
| `activity` | `[{"id","frontend","operations":[op],"started_at","finished_at"\|null,"state","outcomes":[…],"owner_pid","labels":[str]}]`; op = `{"install"\|"remove"\|"upgrade": PackageId}` / `{"upgrade_all":{"backend"}}` / `{"refresh":{"backend"}}` / `{"clean":{"backend","key"}}` | drawer, queued badge, progress-line hiding |
| `background_state` | `{"last_check":epoch,"available":int,"failures":[{"source","kind"}],"notify":bool}` | Settings, notifications, badge |
| `notification_history` | opaque string | persist only |
| `system_approval` | "" or key string | persist; Settings switch |
| `approval_error` | text | Settings |
| `auto_update_result` | `{"updated","failed","total"}` | notification |
| `self_update` | "" \| "automatic" \| "manual" | restart flow |
| `confirmation` | plain text | §5.2 |
| `confirmation_data` | §5.3 | dialogs |
| `opened` | "" or `{"package": row,"description","homepage","publisher","license","dependencies","screenshots","location","action":"Install"\|"Manage"\|""}` | opened page |
| `version` | text | About |
| `busy` | bool | any foreground job (loads, plans, writes) |
| `writing` | bool | a change is running |
| `reading` | bool | only a read/details job runs (rows can still act) |
| `upgradable` | bool | not used by the QML |
| `needs_poll` | bool | keep polling |
| `refreshing` | bool | quiet reload of cached rows |

PackageId JSON: `{"backend","name","architecture","scope","remote"?,"reference"?}`.
Progress/notice "id rows" use row field names: `{"source","name",
"architecture","remote","scope","reference"}` so identity works on them.

### 6.2 Operation progress line ("ActionProgress")
- Shown in the page content when `writing` and `progress.label`, unless the
  Activity drawer is open and shows that same running entry. A second copy
  floats at the bottom of an open app page (inset 16 px).
- Layout: label (≤ 45% width), 6 px bar, count, **Cancel** →
  `controller.cancel()`. Under 460 px the bar moves to its own row.
- Count: `transfer_total > 0` → "NN%"; `total > 1` → "done of total".
- Bar: transfer bytes if known, else done/total; indeterminate when neither
  (sweeping 30% segment, 1.28 s loop; with motion off a still striped bar).
  Fill width animates 200 ms.

### 6.3 Row progress
- A row is active when `writing` and its identity is in `activeRows` or
  `progress.targets`.
- Fraction: `total ≤ 1` → `fraction` (else transfer, else done/total, else
  −1). Batch: finished → 1; running (`current` identity, or no current and
  row source == `current_source`) → transfer fraction or −1 (sweep);
  waiting → 0 (empty track).
- Drawn as a 3 px line at the row bottom (track = color at 18%); −1 sweeps a
  28% segment (1.2 s loop) or rests centered with motion off; fades out
  when done.

### 6.4 Polling
- Timer: interval **40 ms while `busy`, else 200 ms**; runs while
  `busy || needs_poll`; each tick → **`controller.poll()`**. All property
  changes happen inside `poll()` (and inside the calls above). In egui:
  call `poll()` each frame when due and `request_repaint_after(40/200 ms)`
  while either flag is true.

### 6.5 Reactions to property changes
- `rows` changed: if new rows exist and (not `retainUntilDone`, or not busy,
  or new count ≥ retained count) stop retaining and, when not writing,
  flash changed rows (§ below). Then `restoreSelection` next frame.
- `busy` → false: clear `openingInput` next frame if still idle; stop
  retaining; flash changed rows; `restoreSelection`; drop the pre-write
  snapshot unless a write/post-write reload is pending; if a close was
  pending, close the window.
- `writing` → true: snapshot `[installed, candidate, update]` per row
  identity; clear green flashes. → false: `activeRows = []`; on list pages
  run **`reload(true, true)` on the next frame** if not busy and not closing.
- Changed-row flash: package rows whose snapshot state differs → green
  (`success` 18%) for **1 s**.
- `details` changed with a selection and non-empty → fade the page body in
  from 0.55 (120 ms).
- `notice` changed → toast or banner (§5.8).
- `confirmation` changed → §5.2.
- `notification_history` → save setting `notificationHistory`.
- `background_state` → if it has `last_check`, save `lastBackgroundState`
  = `{last_check, available, failures, notify:false}`; update badge and
  maybe notify (§7.4).
- `system_approval` → save setting `systemApproval`.
- `self_update`: "automatic" → after **5 s** (re-armed while writing)
  `restartPkgDeck()`; "manual" → restart toast.
- `restartPkgDeck()`: `controller.restart_app(!windowVisible)`; if true,
  force quit.
- `auto_update_result`, `background_state.notify` → notifications (§7.4).

### 6.6 Timers summary
| Timer | Interval | Action |
| --- | --- | --- |
| poll | 40/200 ms | `controller.poll()` |
| first background check | 30 s once, while `backgroundMode` | `checkUpdates(false)` |
| background check | 300 s repeat, while `backgroundMode` | `checkUpdates(false)` |
| search debounce | 220 ms | `submitSearch()` |
| installed filter debounce | 150 ms (> 400 rows) | apply filter |
| hover warm | 150 ms | `controller.warm_details(raw)` |
| pending delay | 800 ms after a read starts | show "Waiting for …" |
| clock | 30 s while visible | refresh "Checked … ago" |
| completion hold | 1 s | clear green flashes |
| self restart | 5 s (repeat while writing) | `restartPkgDeck()` |
| post-write reload | next frame | `reload(true, true)` |
| toast | 5 s (restart toast 24 h) | hide |
| scrollbar linger | 900 ms | fade handle |

`checkUpdates(force)` → **`controller.check_updates(sources, enabled,
offline, metered, force)`**: sources = `checkedSources().join(",")` (all
known ids when none chosen), enabled = `backgroundMode` setting, offline =
network disconnected or captive portal, metered = metered connection,
force = true only from tray **Check now**. The controller decides if a
check is due (interval, offline, metered, busy).

`backgroundState` shown = `background_state` if not `"{}"`, else the saved
`lastBackgroundState`.

### 6.7 Startup sequence (window creation)
1. `controller.set_check_interval(checkInterval)`,
   `set_auto_update(autoUpdate)`, `set_allow_removals(allowRemovals)`,
   `restore_system_approval(systemApproval)`,
   `restore_notification_history(notificationHistory)`.
2. Enabled sources: CLI `--from ID` / `--from=ID` (repeatable, known ids
   only, session-only), else setting `sourceList`, else legacy `source`.
3. Restore `nameWidth`/`versionWidth` (only if 80..600), `sortColumn` (only
   name/version/status/summary/capabilities), `sortAscending`.
4. `reload()` (Search with empty text: no query). Focus the search field
   next frame if visible.
5. If exactly one CLI argument starts with `file://`, `https://`,
   `flatpak+https://` or `/`: `openExternalInput(it)` next frame.
Later setting changes call `set_check_interval`, `set_auto_update`,
`set_allow_removals` immediately.

### 6.8 Closing
- Close request: if not force-quitting and `backgroundMode` and a tray
  exists → hide to tray instead.
- Else if `busy`: keep open, `closePending = true`, `controller.cancel()`;
  the window closes when busy ends.
- `quitFromKeyboard()` (Ctrl+Q, macOS Quit menu): on macOS always force
  quit; close; if force and nothing pending, quit the app.

---

## 7. Keyboard, platform and startup

### 7.1 Shortcuts (Ctrl = Cmd on macOS)
| Keys | Action | Condition |
| --- | --- | --- |
| Ctrl+1..5 | open Search/Installed/Updates/Clean/Sources | |
| Ctrl+, / Ctrl+6 | open Settings | |
| Ctrl+J | toggle Activity | |
| Esc | close Activity | drawer open |
| Esc | page review → `confirm(false)`; app page → `closePage()`; list focused with selection → close details (deselect); search field with text → clear it | no dialog, drawer closed |
| Back key (Alt+Left; macOS Cmd+[), mouse Back button | `closePage()` | app page open, no page review |
| Ctrl+F | Installed: focus+select filter; Updates/Clean: open source picker and focus its search; else go to Search and focus+select the query (keyboard ring) | |
| Ctrl+L | focus results list | list visible, has rows, not retaining |
| Ctrl+R | `reload(true)` | list pages |
| Ctrl+I / Ctrl+D / Ctrl+U | propose install / remove / upgrade for the selection | `!busy \|\| writing` |
| Ctrl+Shift+U | `upgradeUpdates()` | Updates, ≥ 1 checked, `!busy \|\| writing` |
| Ctrl+M (macOS Ctrl+Shift+R) | `propose("refresh")` | `!busy \|\| writing` |
| Ctrl+Q | `quitFromKeyboard()` | |
| Ctrl+W / Ctrl+M (macOS) | close window / minimize | macOS only |
| Ctrl+Enter, Alt+letter, Alt+C | apply / apply / cancel | confirmation dialog |

List keys (list focused; switch to keyboard mode → show focus ring):
Up/Down ±1, PageUp/PageDown ±10, Home/End, all with `choose(i, false)`;
Space → `choose(current)` (opens the app page); Enter/Return →
`runRowAction(current)`. Down in the search field or Installed filter jumps
into the list at row 0.

### 7.2 Focus behavior
- Startup focuses the Search field (when on Search and visible).
- Streaming results never steal focus from the user's control.
- App page gets focus when it appears; closing it focuses the list.
- Dialog focus: Add link dialog → link field; Add repository → name;
  Confirm → **Cancel**; source picker → its search field.
- Focus rings: 2 px accent for keyboard focus only (Tab/shortcut), never for
  mouse clicks.
- Tab reaches the sidebar resize handle, column headers, details resize
  handle, list, and buttons.

### 7.3 Mac key text
`keys("Ctrl+Shift+U")` → "⇧⌘U": modifiers in the order Alt "⌥", Shift "⇧",
Ctrl "⌘", then the key. Other platforms show the text as written.

### 7.4 Tray, menu bar and notifications
- Tray icon (Linux/other): visible while `backgroundMode` (and available),
  tooltip "PkgDeck", left click toggles show/hide. Menu: **Open**
  (show+raise+activate), **Check now** (`checkUpdates(true)`), **Quit**
  (force quit). Clicking a notification → show window and open Updates.
- macOS: native status item (template icon) instead of Qt's tray, same
  three items; native app menu "Window": About PkgDeck (open Settings),
  Settings… (open Settings), Quit PkgDeck, Minimize, Close Window, Show
  PkgDeck. Clicking the Dock icon while hidden shows the window. Dock badge
  = `background_state.available` when > 0, else cleared. Notification
  permission refreshed when the app becomes active.
- Update notification: when `background_state.notify` and background mode
  and tray and notifications are available → notify "PkgDeck updates" /
  "N update(s) available", then **`controller.acknowledge_notification()`**.
- Auto-update notification: when `auto_update_result.total > 0` (and the
  same conditions) → "PkgDeck updates" / "Installed N update(s)" or "No
  updates installed", + ", F need your attention" when failed > 0.
- Notification timeout on Linux tray: 8 s.

### 7.5 Startup, CLI and single instance (`main.rs`, `native/main.cpp`)
- `--version` prints `pkgdeck {VERSION}` and exits.
- macOS: if launched through a symlink (Homebrew bin), re-exec the real
  binary inside `Contents/MacOS`. Prepend `/opt/homebrew/bin`, `/opt/homebrew/sbin`,
  `/usr/local/bin`, `/usr/local/sbin`, `/opt/local/bin`, `/opt/local/sbin`
  to `PATH` when missing (Finder's minimal PATH).
- App identity: name "PkgDeck", organization "pkgdeck", domain
  "astrovm.github.io", desktop file / bundle id `io.github.astrovm.PkgDeck`.
  Settings migrate once from the legacy "astrovm" organization; old cache
  `~/.cache/astrovm/PkgDeck` is deleted.
- Args: `--background` (start hidden, §1.1), `--from ID` (repeatable),
  `--auth sudo` (dev/tests: use sudo instead of polkit), `--smoke-test`
  (CI: start, verify the tray menu, print `PKGDECK_GUI_READY`, quit),
  first arg starting with `/`, `file://`, `https://`, `flatpak+https://` =
  input to open.
- **Single instance**: local socket
  `$XDG_RUNTIME_DIR/pkgdeck-open-{euid}` (or the platform runtime dir).
  On launch try to connect (200 ms); if a window is listening, send the
  input as UTF-8 + NUL (≤ 8192 bytes; empty input allowed) and exit 0.
  Otherwise listen (user-only access; remove a stale socket). Received
  input → `openExternalInput(input)` (empty skipped) and show/unminimize,
  raise, activate the window. Inputs that arrive before the window exists
  are queued.
- Autostart (`controller.set_autostart`): Linux writes an XDG autostart
  `.desktop` running `pkgdeck --background`; macOS a LaunchAgent
  `~/Library/LaunchAgents/io.github.astrovm.PkgDeck.plist`.
- `controller.restart_app(background)` relaunches the updated install
  (adds `--background` when hidden).
- Images: screenshots/icons over HTTPS use a disk cache (64 MB, entries
  kept ≤ 7 days, 1 day default), prefer-cache, 15 s transfer timeout, no
  less-safe redirects. Local icon paths load as `file://` with spaces
  encoded.

### 7.6 Persisted settings (QSettings, group `Browser`)
| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `appearance` | int | 0 | 0 system, 1 dark, 2 light |
| `reduceMotion` | bool | false | animations off |
| `sourceList` | string | "" | enabled sources CSV; "" = all |
| `source` | string | "" | legacy single source (migrated) |
| `nameWidth` | int | 202 | name column |
| `versionWidth` | int | 150 | version column |
| `sortColumn` | string | "" | name/version/status/summary/capabilities |
| `sortAscending` | bool | true | |
| `backgroundMode` | bool | true | background checks + tray |
| `checkInterval` | int | 30 | minutes |
| `autoUpdate` | bool | false | install updates automatically |
| `allowRemovals` | bool | true | updates may remove packages |
| `systemApproval` | string | "" | helper approval key |
| `autostart` | bool | false | start at login |
| `notificationHistory` | string | "{}" | from controller |
| `lastBackgroundState` | string | "{}" | last background check |
| `sidebarWidth` | int | 212 | |
| `detailsHeight` | int | 0 | app pane height, 0 = auto |
| `flatpakScopeChoices` | string | "{}" | `{groupKey: identity}` |
| `searchHint` | string | "" | last search hint |
Not persisted (session): page source filters, Installed text filter,
Duplicate installs, Show unavailable, Updates unchecked set, expanded
Activity entries.
Files: Linux `~/.config/pkgdeck/PkgDeck.conf`; macOS preferences domain
`io.github.astrovm.PkgDeck`.

---

## 8. Visual design

### 8.1 Theme tokens (Theme.qml)
Appearance 0 follows the system palette for canvas/surface/ink/muted/accent/
accentInk; dark/light fixed values otherwise. System dark = color-scheme
hint dark, or (unknown hint) window color lightness < 0.5.
| Token | Dark | Light | System mode |
| --- | --- | --- | --- |
| canvas | `#0e1015` | `#f4f6f9` | palette window |
| surface | `#161a21` | `#ffffff` | palette base |
| ink | `#e9edf3` | `#18212d` | palette text |
| muted | `#98a3b3` | `#5a6676` | palette placeholder text |
| accent | `#7aa7ff` | `#2f68d8` | palette highlight |
| accentInk | `#0e1015` | `#ffffff` | palette highlighted text |
| danger | `#f28b91` | `#c1343f` | (by dark/light) |
| success | `#6fd39a` | `#1b7a45` | |
| warning | `#f2c56b` | `#9a6400` | |
| heart | `#e34b5f` | `#e34b5f` | |
Derived (alpha of ink/accent): line = ink 12% (light 13%); strongLine = ink
30% (32%); hoverTint = ink 6% (5%); selection = accent 20% (14%).
Shape: smallRadius 5, controlRadius 9, cardRadius 12. Spacing: small 6,
normal 10, large 16, gutter 16, scroll gutter 16. Control height
`max(36, round(pt×2.9))`. Type scale ×: caption 0.8, small 0.9, title 1.25,
heading 1.6. Motion (all 0 when animations are off): feedback 80 ms,
reveal 120 ms, layout 200 ms, pulse 800 ms; easing out-cubic.

### 8.2 Controls
- Buttons: height = control height, radius 9, padding 14 (9 icon-only),
  18 px icon + 9 gap. Default: `surface` + `line` border + hover tint.
  Primary: accent fill (lighter 8% hover, darker 8% pressed), accentInk
  DemiBold. Flat: transparent, glyph-tinted 14% on hover. Pressed scales
  to 0.97. Disabled 42% opacity (custom themes). Tooltip delay 500 ms.
- Switch (settings rows): 38×22 track, accent when on, 16 px knob sliding
  120 ms; whole row toggles; label left, switch right.
- TickBox checkbox: 18 px, radius 5, 1.5 px `strongLine` border; checked =
  accent fill + 13 px check (pops with overshoot).
- Text fields: control height + 2, radius 9, 1 px line → 2 px accent on focus.
- Combo: `down` chevron rotating 180° when open; popup with check on the
  current item.
- Scrollbars: handle only (6 px, muted), 0.42/0.6 hover/0.75 pressed
  opacity, shown while scrolling or hovered, lingering 900 ms; no groove.
- Lists use a 16 px right gutter so the bar never covers row content.

### 8.3 Responsive layout (by page width = window − sidebar)
- Full table ≥ 748; medium (no summary column) < 748; compact cards < 560.
- Header buttons icon-only < 600. Row icons hidden < 420. Updates
  selection buttons icon-only < 360. Page margins 28 (12 under 820 window).

### 8.4 Icons (DeckIcon)
Stroke icons on a 24×24 grid, 1.7 px stroke, round caps/joins, `heart`
filled. Names: heart, package, discover, search, installed (check), up,
down, fwupd, updates (up arrow), sources, settings, help, install, remove
(trash), refresh, cancel (×), activity, bell, filter, warning, external,
right, back, add, launch, info, spinner (open arc), plus per-source
glyphs: apt, homebrew, homebrew-cask, docker, podman, cargo, npm, pnpm,
bun, pip, pipx, uv, mise, composer, gem. Unknown names draw `package`.
Polyline data is in `DeckIcon.qml` (`drawings`, `arcs`) and already ported in
`crates/pkgdeck-egui/src/icons.rs`.

### 8.5 Animations worth keeping (skip all when animations are off)
Page fade-in (0.35→1), nav pill slide, sidebar rail slide, app page slide
(+32 px in, list −24 px back), details fade (0.55→1), list row add/remove
fades and moves (only for < 12 changes; bulk loads replace at once), green
completion flash, row progress sweep, indeterminate bar sweep, skeleton
pulses, results spinner, toast slide/fade, Activity drawer slide and scrim
fade, badge pop, dialog fade+scale, chevron rotations, button press scale.
With animations off: indeterminate bars show still stripes / centered
segments, spinners hide, lists stop at their ends.
