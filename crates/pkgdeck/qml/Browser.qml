import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtQuick.Dialogs
import QtCore
import QtNetwork
import org.kde.kirigami as Kirigami

Controls.ApplicationWindow {
    id: root
    property bool openingInput: false
    function openExternalInput(input) {
        openingInput = true;
        backend.openInput(input);
        if (!backend.busy && !backend.confirmation.length)
            openingInput = false;
    }
    FileDialog {
        id: installationPicker
        title: "Open installation file"
        nameFilters: ["Packages and sources (*.AppImage *.deb *.rpm *.pkg.tar.zst *.pkg.tar.xz *.pkg.tar.gz *.pkg.tar.bz2 *.pkg.tar.lz4 *.flatpak *.flatpakref *.flatpakrepo *.snap *.repo *.sources *.list *.ymp)"]
        onAccepted: root.openExternalInput(selectedFile.toString())
    }
    ThemedDialog {
        id: addPackageDialog
        objectName: "addPackageDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 480)
        title: "Add from file or link"
        modal: true
        standardButtons: Controls.Dialog.Cancel
        onOpened: { packageLink.text = ""; packageLink.forceActiveFocus(); }
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 12
            Controls.TextField {
                id: packageLink
                objectName: "packageLink"
                Layout.fillWidth: true
                placeholderText: "Paste an HTTPS link"
                Accessible.name: "Package or repository link"
                selectByMouse: true
                onAccepted: if (previewLink.enabled) root.openPackageLink()
            }
            RowLayout {
                Layout.fillWidth: true
                ActionButton {
                    objectName: "browsePackageFile"
                    text: "Choose file…"
                    symbol: "package"
                    onClicked: { addPackageDialog.close(); installationPicker.open(); }
                }
                Item { Layout.fillWidth: true }
                ActionButton {
                    id: previewLink
                    objectName: "previewPackageLink"
                    text: "Preview link"
                    symbol: "search"
                    primary: true
                    enabled: /^(?:flatpak\+)?https:\/\/\S+$/.test(packageLink.text.trim())
                    onClicked: root.openPackageLink()
                }
            }
        }
    }
    DropArea {
        anchors.fill: parent
        z: 1000
        onEntered: (drag) => { if (!drag.hasUrls || drag.urls.length !== 1) drag.accepted = false; }
        onDropped: (drop) => {
            if (drop.urls.length === 1) {
                root.openExternalInput(drop.urls[0].toString());
                drop.acceptProposedAction();
            } else drop.accepted = false;
        }
    }
    SystemPalette { id: systemPalette }
    SystemPalette { id: disabledPalette; colorGroup: SystemPalette.Disabled }
    required property var backend
    readonly property var repositoryReport: JSON.parse(backend.repositories || "{}")
    function repositoryChange(row, action, extra) {
        const request = Object.assign({backend: row.backend, name: row.name, scope: row.scope, action: action}, extra || {});
        backend.changeRepository(JSON.stringify(request));
    }
    property string currentView: "Search"
    readonly property var activityRows: JSON.parse(backend.activity || "[]")
    readonly property int queuedCount: activityRows.filter(row => row.state === "queued").length
    readonly property var backgroundState: JSON.parse(backend.background_state || "{}")
    property bool startHidden: Qt.application.arguments.indexOf("--background") >= 0
    property bool forceQuit: false
    property bool trayAvailable: false
    property alias backgroundMode: preferences.backgroundMode
    property alias autostartEnabled: preferences.autostart
    function checkUpdates(force) {
        const offline = NetworkInformation.reachability === NetworkInformation.Reachability.Disconnected || NetworkInformation.isBehindCaptivePortal;
        backend.checkUpdates(root.checkedSources().join(","), preferences.backgroundMode, offline, NetworkInformation.isMetered, force === true);
    }
    property string resultView: "Search"
    property var liveItems: JSON.parse(backend.rows || "[]")
    property var retainedItems: []
    property bool retainingResults: false
    property string resultQuery: ""
    property var items: currentView === resultView ? (retainingResults ? retainedItems : liveItems) : []
    property bool reduceMotion: preferences.reduceMotion
    readonly property bool motionEnabled: !reduceMotion && Kirigami.Units.shortDuration > 0
    readonly property int feedbackDuration: motionEnabled ? Math.round(Kirigami.Units.shortDuration * 0.8) : 0
    readonly property int revealDuration: motionEnabled ? Math.round(Kirigami.Units.shortDuration * 1.2) : 0
    property var revealedRows: new Set()
    property var beforeWrite: null
    property var completedRows: []
    function revealRow(row) {
        const identity = rowIdentity(row);
        if (revealedRows.has(identity))
            return false;
        revealedRows.add(identity);
        return motionEnabled;
    }
    function packageState(row) {
        return JSON.stringify([row.installed, row.candidate, row.update]);
    }
    function markChangedRows() {
        if (beforeWrite === null)
            return;
        const changed = liveItems.filter((row) => row.kind === "package"
            && beforeWrite[rowIdentity(row)] !== undefined
            && beforeWrite[rowIdentity(row)] !== packageState(row)).map(rowIdentity);
        if (changed.length > 0) {
            completedRows = changed;
            completionHold.restart();
        }
    }
    onReduceMotionChanged: preferences.reduceMotion = reduceMotion
    onMotionEnabledChanged: {
        if (!motionEnabled) {
            detailsReveal.complete();
            completedRows = [];
        }
    }
    property var detail: JSON.parse(backend.details || "{}")
    readonly property bool detailMatchesSelection: selected !== null && detail.package !== undefined && rowIdentity(selected) === rowIdentity(detail.package)
    readonly property var screenshots: detailMatchesSelection ? (detail.screenshots || []) : []
    property var failedScreenshots: []
    readonly property var visibleScreenshots: screenshots.filter(image => failedScreenshots.indexOf(image.url) < 0)
    onScreenshotsChanged: failedScreenshots = []
    function hideFailedScreenshot(url, identity) {
        if (rowIdentity(selected) === identity && failedScreenshots.indexOf(url) < 0)
            failedScreenshots = failedScreenshots.concat([url]);
    }
    readonly property string selectedIcon: (detailMatchesSelection && detail.package.icon) || (selected && selected.icon) || ""
    function detailText() {
        if (detail && detail.cleanup)
            return [detail.cleanup.summary || "", detail.cleanup.preview || ""].filter(Boolean).join("\n\n");
        if (detailMatchesSelection) {
            const description = (detail.description || "").trim();
            return description === (selected.summary || "").trim() ? "" : description;
        }
        if (detail && detail.failure)
            return (detail.failure.error || "") + "\n" + (detail.hint || "");
        return detail && detail.source ? ((detail.availability || "") + "\nLast successful check: " + lastSuccessfulCheck(detail.source)) : "";
    }
    property string screenshotUrl: ""
    property string screenshotCaption: ""
    property var focusStack: []
    function rememberDialogFocus() { focusStack = focusStack.concat([root.activeFocusItem]); }
    function restoreDialogFocus() {
        const returnFocusItem = focusStack.length ? focusStack[focusStack.length - 1] : null;
        focusStack = focusStack.slice(0, -1);
        try {
            if (returnFocusItem && returnFocusItem.visible && returnFocusItem.enabled) {
                returnFocusItem.forceActiveFocus();
                return;
            }
        } catch (e) { /* A reused result delegate may have been destroyed. */ }
        results.forceActiveFocus();
    }
    property bool closePending: false
    property bool queryDirty: false
    property var selectedIdentity: null
    function rowIdentity(row) {
        return row ? JSON.stringify([row.source, row.name, row.architecture, row.remote || null, row.scope, row.reference || null]) : "";
    }
    property var selected: !retainingResults && results.currentIndex >= 0 && results.currentIndex < viewItems.length ? viewItems[results.currentIndex] : null
    // Persistent manager enablement. Empty means every known manager;
    // page filters below never change this preference.
    property string sourceSelection: ""
    property var viewSourceFilters: ({})
    readonly property var sourceCatalog: JSON.parse(backend.source_catalog || "[]")
    onSourceCatalogChanged: {
        if (sourcePopup.visible && sourcePopup.draftSources.length === 0)
            sourcePopup.draftSources = root.effectiveSources().filter((id) => root.sourceInfo(id).availability_kind === "available" && root.sourceSupportsView(id));
    }
    readonly property var reportState: JSON.parse(backend.report_state || "{}")
    readonly property var readFailures: {
        if (currentView !== resultView)
            return [];
        const failures = reportState.failures && reportState.failures.length ? reportState.failures :
            items.filter((row) => row.kind === "failure" && row.failure_kind !== "unsupported")
                .map((row) => ({source: row.source, kind: row.failure_kind || "failed"}));
        return failures.filter((failure) => effectiveSources().indexOf(failure.source) >= 0);
    }
    property string expandedFailure: ""
    function failureSummary(id) {
        const row = items.find((item) => item.kind === "failure" && item.source === id);
        const reportFailure = (reportState.failures || []).find((failure) => failure.source === id);
        return (row && row.summary) || (reportFailure && reportFailure.detail) || (currentView === "Sources" ? sourceInfo(id).summary : "") || "Source check did not complete.";
    }
    function lastSuccessfulCheck(id) {
        const seconds = (reportState.last_success || {})[id];
        return seconds ? new Date(seconds * 1000).toLocaleString() : "Not checked successfully this session";
    }
    function copyableDiagnostics() {
        return "View: " + currentView + "\nState: " + (reportState.phase || "unknown") + "\n" +
            readFailures.map((failure) => failure.source + ": " + failure.kind + " — " + failureSummary(failure.source)).join("\n");
    }
    function failureHelp(kind) {
        if (kind === "authorization")
            return "Retry and approve the system prompt.";
        if (kind === "locked")
            return "Wait for the package manager to finish, then retry.";
        if (kind === "unsupported")
            return "This source does not support this page.";
        return "Retry this source. If it still fails, check that its package manager is working.";
    }
    function retryFailedSource(id) {
        if (currentView === "Search" && queryDirty)
            return;
        const query = currentView === "Installed" ? "" : searchPane.text;
        backend.retrySource(currentView, query, id);
        sourceFailuresDialog.close();
    }
    function clearVisibleFilters() {
        installedFilter = "";
        multiSourceOnly = false;
        const filters = Object.assign({}, viewSourceFilters);
        const hadSourceFilter = filters[currentView] !== undefined;
        delete filters[currentView];
        viewSourceFilters = filters;
        if (hadSourceFilter)
            reload();
    }
    function emptyStateMessage() {
        if (backend.writing && currentView === "Search" && items.length === 0)
            return "Search will resume shortly.";
        if (backend.busy || reportState.phase === "loading")
            return "";
        if (readFailures.length > 0)
            return reportState.phase === "partial" ? "No results from the sources that completed." : "Could not check these sources.";
        if (reportState.phase === "unsupported")
            return "No enabled sources support this view.";
        if (currentView === "Search" && queryDirty)
            return "";
        if (currentView === "Search" && searchPane.text.trim().length === 0)
            return "";
        if (currentView === "Installed" && (installedFilter.length > 0 || multiSourceOnly))
            return "No packages match these filters.";
        if (viewSourceFilters[currentView] && items.length > 0)
            return "No results from selected sources.";
        if (reportState.phase === "cached" || reportState.phase === "stale")
            return currentView === "Updates" ? "No updates in the last check." : "No results in the last check.";
        if (currentView === "Updates")
            return reportState.phase === "complete" ? "You're up to date" : "No updates to show.";
        if (currentView === "Clean")
            return "Nothing to clean";
        if (currentView === "Installed")
            return "No installed packages.";
        if (currentView === "Sources")
            return "No available sources.";
        return currentView === "Search" ? "No matching packages." : "No results to show.";
    }
    function resultsHeading() {
        if (backend.writing)
            return "Applying changes…";
        if (backend.busy)
            return retainingResults ? "Refreshing…" :
                (currentView === "Search" ? "Searching…" : "Loading…");
        const count = viewItems.length;
        const noun = currentView === "Sources" ? "source" : currentView === "Clean" ? "cleanup task" :
            currentView === "Updates" ? "update" : "package";
        return count + " " + noun + (count === 1 ? "" : "s");
    }
    property string installedFilter: ""
    property bool showUnavailableSources: false
    // Session-only filter showing just the apps installed from more than
    // one source. Session-only like the text filter: view state, not a
    // persisted preference.
    property bool multiSourceOnly: false
    property bool useSudo: argument("--auth", preferences.authorization) === "sudo"
    function updateOnly(source) {
        return ["fwupd", "codex", "claude", "grok", "opencode"].indexOf(source) >= 0;
    }
    readonly property var knownSourceIds: ["apt", "dnf", "pacman", "zypper", "snap", "homebrew", "homebrew-cask", "appimage", "flatpak", "docker", "podman", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem", "fwupd", "codex", "claude", "grok", "opencode"]
    readonly property var sourceIds: knownSourceIds.concat(sourceCatalog.map((row) => row.source).filter((id) => knownSourceIds.indexOf(id) < 0))
    readonly property var sourceNames: ["APT", "DNF", "Pacman", "Zypper", "Snap", "Homebrew", "Homebrew Casks", "AppImage", "Flatpak", "Docker images", "Podman images", "Cargo", "npm", "pnpm", "Bun", "pip", "pipx", "uv", "Composer", "RubyGems", "Firmware", "Codex (standalone)", "Claude Code (standalone)", "Grok (standalone)", "OpenCode (standalone)"]
    function containerSource(source) {
        return source === "docker" || source === "podman";
    }
    function checkedSources() {
        if (sourceSelection === "")
            return sourceIds.slice();
        const checked = sourceSelection.split(",").filter((id) => sourceIds.indexOf(id) >= 0);
        return checked.length > 0 ? checked : sourceIds.slice();
    }
    function setManagerEnabled(id, enabled) {
        let selected = checkedSources().filter((source) => source !== id);
        if (enabled)
            selected.push(id);
        if (selected.length === 0)
            return;
        sourceSelection = selected.length >= sourceIds.length ? "" : sourceIds.filter((source) => selected.indexOf(source) >= 0).join(",");
        preferences.sourceList = sourceSelection;
        preferences.source = "";
        viewSourceFilters = ({});
        reload();
    }
    function openPackageLink() {
        const input = packageLink.text.trim();
        if (!/^(?:flatpak\+)?https:\/\/\S+$/.test(input))
            return;
        addPackageDialog.close();
        root.openExternalInput(input);
    }
    function toggleSourcePopup() {
        if (sourcePopup.visible) {
            sourcePopup.close();
            return;
        }
        backend.checkSources();
        rememberDialogFocus();
        sourcePopup.open();
    }
    function effectiveSources(view) {
        const enabled = checkedSources();
        const selected = viewSourceFilters[view || currentView];
        return selected ? enabled.filter((id) => selected.indexOf(id) >= 0) : enabled;
    }
    function checkedCsv() {
        const checked = effectiveSources();
        return checked.length >= sourceIds.length ? "" : checked.join(",");
    }
    function sourceSummary() {
        const checked = effectiveSources();
        if (checked.length === 1)
            return sourceDisplayName(checked[0]);
        if (!viewSourceFilters[currentView])
            return checkedSources().length >= sourceIds.length ? "Available sources" : checked.length + " enabled sources";
        return checked.length + " sources";
    }
    function sourceInfo(id) {
        return sourceCatalog.find((row) => row.source === id) || {source: id, summary: sourceCatalog.length ? "Unsupported on this platform" : "Checking availability…", availability_kind: sourceCatalog.length ? "platform" : "checking", capabilities: []};
    }
    function sourceCategory(id) {
        if (["apt", "dnf", "pacman", "zypper", "fwupd"].indexOf(id) >= 0)
            return "System";
        if (["snap", "homebrew", "homebrew-cask", "appimage", "flatpak"].indexOf(id) >= 0)
            return "Applications";
        if (containerSource(id))
            return "Containers";
        return "Developer tools";
    }
    function sourceSupportsView(id) {
        const capability = ({"Search":"search", "Installed":"installed", "Updates":"upgrade", "Clean":"clean"})[currentView];
        return !capability || sourceInfo(id).capabilities.indexOf(capability) >= 0;
    }
    function pickerItems() {
        const query = sourcePopup.searchText.trim().toLowerCase();
        const categories = ["System", "Applications", "Developer tools", "Containers"];
        const discovered = sourceCatalog.map((row) => row.source).filter((id) => sourceIds.indexOf(id) >= 0);
        const ids = discovered.slice();
        if (sourcePopup.showUnavailable) {
            for (const id of sourceIds) {
                if (ids.indexOf(id) < 0)
                    ids.push(id);
            }
        }
        const matches = ids.filter((id) => !query || (sourceDisplayName(id) + " " + id).toLowerCase().indexOf(query) >= 0);
        const available = matches.filter((id) => sourceInfo(id).availability_kind === "available" && sourceSupportsView(id));
        const unavailable = matches.filter((id) => available.indexOf(id) < 0);
        const ordered = [];
        for (const group of categories)
            ordered.push(...available.filter((id) => sourceCategory(id) === group));
        if (sourcePopup.showUnavailable)
            ordered.push(...unavailable);
        return ordered;
    }
    function toggleDraftSource(id) {
        let checked = sourcePopup.draftSources.slice();
        const at = checked.indexOf(id);
        if (at >= 0) {
            if (checked.length <= 1)
                return;
            checked.splice(at, 1);
        } else {
            checked.push(id);
        }
        sourcePopup.draftSources = checked;
    }
    function applySourceDraft() {
        const selected = sourcePopup.draftSources;
        const filters = Object.assign({}, viewSourceFilters);
        const enabled = checkedSources();
        const filtered = enabled.filter((id) => selected.indexOf(id) >= 0);
        if (filtered.length >= enabled.length)
            delete filters[currentView];
        else
            filters[currentView] = filtered;
        viewSourceFilters = filters;
        sourcePopup.close();
        if (["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(currentView) >= 0)
            reload();
    }
    // Delegate lookup by position in sourceIds. Popup content reparents to
    // the Overlay, so findChild cannot reach the checkboxes; the Repeater
    // hands out the live delegate for real clicks in tests.
    function sourceCheckAt(index) {
        const id = sourceIds[index];
        for (let i = 0; i < checklistRepeater.count; i++) {
            const item = checklistRepeater.itemAt(i);
            if (item && item.sourceId === id)
                return item.checkBox;
        }
        return null;
    }
    // Deselected package identities for the Updates multi-select. Every
    // row is checked by default; deselections (not selections) are stored
    // so newly streamed rows start checked too. Identities, not indexes:
    // streaming partials re-sort rows, so the controller re-resolves each
    // identity and skips stale ones, never guessing.
    property var uncheckedPackages: []
    readonly property var uncheckedIdentitySet: new Set(uncheckedPackages)
    function packageChecked(row) {
        return !uncheckedIdentitySet.has(rowIdentity(row));
    }
    function togglePackage(row) {
        const id = rowIdentity(row);
        if (!id)
            return;
        const unchecked = uncheckedPackages.slice();
        const at = unchecked.indexOf(id);
        if (at >= 0)
            unchecked.splice(at, 1);
        else
            unchecked.push(id);
        uncheckedPackages = unchecked;
    }
    readonly property var allPackageIdentities: {
        const all = [];
        const seen = new Set();
        const rows = root.items;
        for (let i = 0; i < rows.length; i++) {
            if (rows[i].kind === "package") {
                const id = rowIdentity(rows[i]);
                if (id && !seen.has(id)) {
                    seen.add(id);
                    all.push(id);
                }
            }
        }
        return all;
    }
    function packageIdentities() {
        return allPackageIdentities;
    }
    readonly property var checkedPackageIds: {
        const unchecked = uncheckedIdentitySet;
        return allPackageIdentities.filter((id) => !unchecked.has(id));
    }
    function checkedIdentities() {
        return checkedPackageIds;
    }
    function selectedCount() {
        return checkedIdentities().length;
    }
    function selectNonePackages() {
        uncheckedPackages = packageIdentities();
    }
    // Nothing deselected: batch per backend like Upgrade all; otherwise
    // upgrade exactly the checked rows.
    function upgradeUpdates() {
        if (uncheckedPackages.length === 0)
            root.propose("upgrade-all");
        else
            backend.proposeChecked(JSON.stringify(checkedIdentities()));
    }
    // Column widths (drag the header gutter) and the active sort. Sorting
    // is QML-side over a copied array: the backend keeps its own order,
    // so selection and actions map the visible index back to the raw one.
    property int nameWidth: 202
    property int versionWidth: 150
    property string sortColumn: ""
    property bool sortAscending: true
    onNameWidthChanged: preferences.nameWidth = root.nameWidth
    onVersionWidthChanged: preferences.versionWidth = root.versionWidth
    onSortColumnChanged: preferences.sortColumn = root.sortColumn
    onSortAscendingChanged: preferences.sortAscending = root.sortAscending
    function sortArrow(column) {
        return sortColumn === column ? (sortAscending ? " ▲" : " ▼") : "";
    }
    function cycleSort(column) {
        if (sortColumn === column)
            sortAscending = !sortAscending;
        else {
            sortColumn = column;
            sortAscending = true;
        }
    }
    function sortValue(column, row) {
        if (column === "version")
            return row.installed || row.candidate || "";
        if (column === "summary" || column === "status")
            return row.summary || "";
        if (column === "capabilities")
            return (row.capabilities || []).join(", ");
        return row.name || "";
    }
    // Best-match-first score for the Search view: exact name, name prefix,
    // name substring, summary prefix, summary substring, then anything the
    // backend returned through another field. Unverifiable offers (a package
    // row with neither an installed nor a candidate version, e.g. a dev
    // backend's uninstalled-name guess) always sort below verified rows so
    // real packages are never buried under names that may not exist. Ties
    // break by name and source so streaming partials settle into a stable
    // order.
    // Missing metadata identifies explicit-install placeholders. Empty
    // version strings from native catalogs are valid search results.
    function isInstalled(row) {
        return row && row.installed !== null && row.installed !== undefined;
    }
    function isFabricated(row) {
        return row.kind === "package" && !isInstalled(row) && (row.candidate === null || row.candidate === undefined);
    }
    function relevanceScore(row, query) {
        const name = (row.name || "").toLowerCase();
        const summary = (row.summary || "").toLowerCase();
        if (name === query || (row.display_name || "").toLowerCase() === query
                || (row.source === "flatpak" && name.split(".").pop() === query))
            return 0;
        if (name.indexOf(query) === 0)
            return 1;
        if (name.indexOf(query) >= 0)
            return 2;
        if (summary.indexOf(query) === 0)
            return 3;
        if (summary.indexOf(query) >= 0)
            return 4;
        return 5;
    }
    function relevanceTiebreak(a, b) {
        if (a.name !== b.name)
            return a.name < b.name ? -1 : 1;
        if (a.source !== b.source)
            return a.source < b.source ? -1 : 1;
        return 0;
    }
    function groupInstalledRows(rows) {
        const members = new Map();
        for (const row of rows) {
            if (row.kind === "package" && row.same_app_group) {
                if (!members.has(row.same_app_group))
                    members.set(row.same_app_group, []);
                members.get(row.same_app_group).push(row);
            }
        }
        const emitted = new Set();
        const grouped = [];
        for (const row of rows) {
            const key = row.same_app_group;
            const group = key ? members.get(key) : null;
            if (!group || group.length < 2) {
                grouped.push(row);
                continue;
            }
            if (emitted.has(key))
                continue;
            emitted.add(key);
            const title = group.reduce((best, member) => member.name.length < best.length ? member.name : best, group[0].name);
            const sources = [...new Set(group.map((member) => root.sourceDisplayName(member.source)))];
            for (let i = 0; i < group.length; i++)
                grouped.push(Object.assign({}, group[i], {groupStart: i === 0, groupTitle: title, groupCount: group.length, groupSources: sources}));
        }
        return grouped;
    }
    readonly property var cleanupFailures: currentView === "Clean" ? items.filter(row => row.kind === "failure") : []
    property var viewItems: {
        let rows = root.currentView === "Search" ? items.filter((row) => row.kind !== "failure" && !isFabricated(row)) : items.filter((row) => row.kind !== "failure");
        if (root.currentView === "Clean")
            rows = rows.filter(row => row.kind === "cleanup");
        if (root.currentView === "Sources" && !root.showUnavailableSources)
            rows = rows.filter(row => row.kind !== "source" || row.available);
        if (root.currentView !== "Sources")
            rows = rows.filter((row) => root.effectiveSources().indexOf(row.source) >= 0);
        // The Installed filter narrows the loaded rows as you type; the
        // backend is queried once with an empty query (see reload).
        if (root.currentView === "Installed") {
            const filter = root.installedFilter.trim().toLowerCase();
            if (filter !== "") {
                const matchingGroups = new Set(rows.filter((row) => row.kind === "package" && ((row.name || "") + " " + (row.summary || "") + " " + (row.source || "")).toLowerCase().indexOf(filter) >= 0).map((row) => row.same_app_group).filter(Boolean));
                rows = rows.filter((row) => row.kind !== "package" || matchingGroups.has(row.same_app_group) || ((row.name || "") + " " + (row.summary || "") + " " + (row.source || "")).toLowerCase().indexOf(filter) >= 0);
            }
            if (root.multiSourceOnly)
                rows = rows.filter((row) => row.kind !== "package" || ((row.same_app_from || []).length > 0));
        }
        if (sortColumn !== "") {
            const column = sortColumn;
            const dir = sortAscending ? 1 : -1;
            rows.sort((a, b) => {
                const x = sortValue(column, a).toLowerCase();
                const y = sortValue(column, b).toLowerCase();
                // Total order: the engine sort is not stable for equal
                // keys, so same-name rows from several sources would flip
                // on every keystroke without a direction-aware tiebreak.
                return (x < y ? -dir : (x > y ? dir : 0)) || relevanceTiebreak(a, b) * dir;
            });
        } else if (root.currentView === "Sources") {
            // Keep enabled managers first so the sources in use are easy to find.
            const enabled = root.checkedSources();
            rows.sort((a, b) => (Number(enabled.indexOf(b.source) >= 0) - Number(enabled.indexOf(a.source) >= 0)) || relevanceTiebreak(a, b));
        } else if (root.currentView === "Search") {
            const query = searchPane.text.trim().toLowerCase();
            if (query !== "")
                rows.sort((a, b) => ((isFabricated(a) ? 1 : 0) - (isFabricated(b) ? 1 : 0)) || (relevanceScore(a, query) - relevanceScore(b, query)) || relevanceTiebreak(a, b));
        }
        return root.currentView === "Installed" ? groupInstalledRows(rows) : rows;
    }
    // The visible index addresses viewItems; the backend addresses items.
    // Filtering, relevance ranking, and column sorts all reorder or narrow
    // the rows, so always resolve through the row identity. Duplicate rows
    // resolve to the first raw match: identical rows are one engine identity
    // shown twice, so either index acts on the same one.
    function originalIndex(visible) {
        if (visible < 0 || visible >= viewItems.length)
            return -1;
        const id = rowIdentity(viewItems[visible]);
        for (let i = 0; i < items.length; i++) {
            if (rowIdentity(items[i]) === id)
                return i;
        }
        return -1;
    }
    readonly property bool compact: width < 960
    readonly property bool navigationCollapsed: width < 820
    readonly property int shortListLimit: compact ? 3 : 8
    function shortResultsHeight() {
        const rowHeight = (row) => (compact
            ? (row.kind === "source" ? Math.max(68, font.pointSize * 5.5) : Math.max(94, font.pointSize * 8.5))
            : Math.max(56, font.pointSize * 5)) + (row.groupStart ? 38 : 0);
        return Math.max(130, (compact ? 60 : 85) + viewItems.reduce((height, row) => height + rowHeight(row), 0));
    }
    readonly property bool systemAppearance: preferences.appearance === 0
    readonly property bool dark: preferences.appearance === 1 || (systemAppearance && Qt.styleHints.colorScheme === Qt.Dark)
    readonly property color canvas: systemAppearance ? systemPalette.window : (dark ? "#0c0e12" : "#f3f5f8")
    readonly property color surface: systemAppearance ? systemPalette.base : (dark ? "#101014" : "#ffffff")
    readonly property color ink: systemAppearance ? systemPalette.text : (dark ? "#ecf1f8" : "#1c2b3e")
    readonly property color muted: systemAppearance ? systemPalette.placeholderText : (dark ? "#a2b1c4" : "#57677e")
    readonly property color line: systemAppearance ? systemPalette.mid : (dark ? "#2a2e37" : "#dce3ec")
    readonly property color accent: systemAppearance ? systemPalette.highlight : (dark ? "#80b6ff" : "#245fc6")
    readonly property color selection: systemAppearance ? Qt.rgba(systemPalette.highlight.r, systemPalette.highlight.g, systemPalette.highlight.b, 0.24) : (dark ? "#1a2740" : "#e8f0ff")
    color: canvas
    palette.window: canvas
    palette.base: surface
    palette.text: ink
    palette.windowText: systemAppearance ? systemPalette.windowText : ink
    palette.buttonText: systemAppearance ? systemPalette.buttonText : ink
    palette.button: systemAppearance ? systemPalette.button : surface
    palette.highlight: accent
    palette.highlightedText: systemAppearance ? systemPalette.highlightedText : (dark ? "#111820" : "#ffffff")

    component ActionButton: Controls.Button {
        id: control
        property color glyphColor: root.ink
        property bool primary: false
        property bool navigation: false
        property string symbol: "package"
        property url iconSource: ""
        property string tooltipText: ""
        Accessible.name: text
        Controls.ToolTip.visible: hovered && tooltipText.length > 0
        Controls.ToolTip.text: tooltipText
        implicitHeight: Math.max(38, root.font.pointSize * 3)
        horizontalPadding: 16
        verticalPadding: 4
        opacity: enabled || root.systemAppearance ? 1 : 0.45
        scale: down ? 0.98 : 1
        Behavior on opacity { NumberAnimation { duration: root.feedbackDuration } }
        Behavior on scale { NumberAnimation { duration: root.feedbackDuration; easing.type: Easing.OutCubic } }
        background: Rectangle {
            radius: 7
            Behavior on color { ColorAnimation { duration: root.feedbackDuration } }
            color: !control.enabled && root.systemAppearance ? disabledPalette.button : control.primary && control.enabled ? root.accent : (control.hovered ? root.selection : root.surface)
            border.color: control.activeFocus ? root.accent : !control.enabled && root.systemAppearance ? disabledPalette.mid : (control.navigation ? "transparent" : root.line)
            border.width: control.activeFocus ? 2 : 1
        }
        contentItem: Item {
            implicitWidth: buttonContents.implicitWidth
            implicitHeight: buttonContents.implicitHeight
            RowLayout {
                id: buttonContents
                x: control.navigation ? 0 : (parent.width - width) / 2
                anchors.verticalCenter: parent.verticalCenter
                width: control.navigation ? parent.width : implicitWidth
                height: implicitHeight
                spacing: 8
                DeckIcon {
                    name: control.symbol
                    visible: control.symbol.length > 0
                    ink: !control.enabled && root.systemAppearance ? disabledPalette.buttonText : control.primary && control.enabled ? root.palette.highlightedText : control.glyphColor
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    Layout.alignment: Qt.AlignVCenter
                }
                Image {
                    objectName: control.iconSource.toString().length > 0 ? control.objectName + "Icon" : ""
                    source: control.iconSource
                    visible: control.iconSource.toString().length > 0
                    sourceSize.width: Math.ceil(width * (root.screen ? root.screen.devicePixelRatio : 1))
                    sourceSize.height: Math.ceil(height * (root.screen ? root.screen.devicePixelRatio : 1))
                    fillMode: Image.PreserveAspectFit
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    Layout.alignment: Qt.AlignVCenter
                    Accessible.ignored: true
                }
                Text {
                    text: control.text
                    visible: text.length > 0
                    font: control.font
                    color: !control.enabled && root.systemAppearance ? disabledPalette.buttonText : control.primary && control.enabled ? root.palette.highlightedText : root.ink
                    Layout.fillWidth: control.navigation
                    Layout.alignment: Qt.AlignVCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }
    }

    // Shared checkbox indicator: transparent box with an accent check.
    // Both the source checklist and the Updates multi-select use it so the
    // two stay visually identical; positioning comes from the instance.
    component TickBox: Rectangle {
        required property bool ticked
        implicitWidth: 20
        implicitHeight: 20
        color: "transparent"
        border.color: root.line
        radius: 4
        DeckIcon {
            name: "installed"
            ink: root.accent
            anchors.centerIn: parent
            width: 14
            height: 14
            visible: ticked
        }
    }

    component ThemedDialog: Controls.Dialog {
        id: dialog
        palette.window: root.surface
        palette.base: root.surface
        palette.text: root.ink
        palette.windowText: root.ink
        palette.button: root.surface
        palette.buttonText: root.ink
        background: Rectangle { color: root.surface; radius: 12; border.color: root.line }
        header: Item {
            implicitHeight: 54
            Controls.Label {
                anchors.left: parent.left
                anchors.leftMargin: 16
                anchors.verticalCenter: parent.verticalCenter
                text: dialog.title
                color: root.ink
                font.bold: true
            }
            ActionButton {
                anchors.right: parent.right
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                text: ""
                symbol: "cancel"
                tooltipText: "Close"
                Accessible.name: "Close dialog"
                onClicked: dialog.reject()
            }
        }
        footer: Controls.DialogButtonBox {
            standardButtons: dialog.standardButtons
            alignment: Qt.AlignRight
            padding: 8
            spacing: 8
            background: Rectangle { color: root.surface }
            delegate: ActionButton { symbol: "" }
            onAccepted: dialog.accept()
            onRejected: dialog.reject()
        }
    }

    component SettingCheckBox: Controls.CheckBox {
        id: setting
        implicitHeight: Math.max(32, root.font.pointSize * 2.5)
        indicator: TickBox {
            x: 0
            y: (setting.height - height) / 2
            ticked: setting.checked
        }
        contentItem: Text {
            text: setting.text
            color: setting.enabled ? root.ink : root.muted
            font: setting.font
            verticalAlignment: Text.AlignVCenter
            leftPadding: 28
        }
    }

    component ThemedComboBox: Controls.ComboBox {
        id: combo
        implicitHeight: 38
        background: Rectangle {
            color: root.surface
            radius: 8
            border.color: combo.activeFocus ? root.accent : root.line
            border.width: combo.activeFocus ? 2 : 1
        }
        contentItem: Controls.TextField {
            text: combo.displayText
            color: combo.enabled ? root.ink : root.muted
            enabled: false
            readOnly: true
            selectByMouse: false
            background: null
            clip: true
            verticalAlignment: Text.AlignVCenter
            leftPadding: 14
            rightPadding: 8
        }
        delegate: Controls.ItemDelegate {
            required property var modelData
            required property int index
            width: ListView.view.width
            text: modelData
            font: combo.font
            highlighted: combo.highlightedIndex === index
            background: Rectangle {
                color: highlighted ? root.selection : "transparent"
            }
            contentItem: Text {
                text: modelData
                color: root.ink
                elide: Text.ElideRight
                verticalAlignment: Text.AlignVCenter
                leftPadding: 14
            }
        }
        popup: Controls.Popup {
            y: combo.height
            width: combo.width
            implicitHeight: Math.min(contentItem.implicitHeight, 320)
            padding: 4
            contentItem: ListView {
                clip: true
                implicitHeight: contentHeight
                model: combo.popup.visible ? combo.delegateModel : null
                currentIndex: combo.highlightedIndex
                delegate: combo.delegate
                Controls.ScrollIndicator.vertical: Controls.ScrollIndicator { }
            }
            background: Rectangle {
                color: root.surface
                radius: 8
                border.color: root.line
            }
        }
    }

    // Display names for backend ids used by related-install indicators without
    // changing the rows' exact identities.
    function sourceDisplayName(id) {
        const at = knownSourceIds.indexOf(id);
        return at >= 0 ? sourceNames[at] : id;
    }
    function sameAppNames(row) {
        return ((row && row.same_app_from) || []).map((id) => root.sourceDisplayName(id));
    }
    function sameAppSummary(row) {
        const names = sameAppNames(row);
        return names.length > 0 ? "Also installed from: " + names.join(", ") : "";
    }
    function versionText(row) {
        if (row.kind === "cleanup")
            return row.cleanup_kind === "orphan_dependencies" ? "Dependencies" : "Cache";
        if (row.kind === "failure")
            return "Failed";
        if (row.kind === "source")
            return row.available ? "Available" : "Unavailable";
        if (row.update === "available") {
            if (!row.installed || !row.candidate || row.installed === row.candidate)
                return row.installed || row.candidate || "—";
            return row.installed + " → " + row.candidate;
        }
        if (isInstalled(row))
            return row.installed || "—";
        return row.candidate || "Unknown";
    }
    // Local icon files become file:// URLs. Paths come from the backend and
    // may contain spaces, which raw concatenation would leave unencoded and
    // unloadable (hiding the fallback source icon with a blank gap).
    function iconUrl(path) {
        return path ? (path.indexOf("https://") === 0 ? path : "file://" + encodeURI(path)) : "";
    }

    property url logoIconSource: "qrc:/pkgdeck/logo.svg"
    property url repositoryIconSource: dark ? "qrc:/pkgdeck/github-dark.svg" : "qrc:/pkgdeck/github.svg"
    readonly property url repositoryUrl: "https://github.com/astrovm/PkgDeck"
    width: 1100
    height: 760
    minimumWidth: 360
    minimumHeight: 400
    visible: !startHidden || !preferences.backgroundMode || !trayAvailable
    title: "PkgDeck — " + currentView + (backend.busy ? " — Working" : "")
    function argument(name, fallback) {
        const index = Qt.application.arguments.indexOf(name);
        return index >= 0 ? Qt.application.arguments[index + 1] : fallback;
    }
    function collectArguments(name) {
        const found = [];
        const args = Qt.application.arguments;
        for (let i = 0; i < args.length; i++) {
            if (args[i] === name && i + 1 < args.length)
                found.push(args[i + 1]);
            else if (args[i].indexOf(name + "=") === 0)
                found.push(args[i].slice(name.length + 1));
        }
        return found;
    }
    function openView(view) {
        const changed = currentView !== view;
        queryDirty = false;
        selectedIdentity = null;
        uncheckedPackages = [];
        if (changed) {
            retainingResults = false;
            retainedItems = [];
            revealedRows = new Set();
            completedRows = [];
        }
        currentView = view;
        results.currentIndex = -1;
        if (view === "Search") {
            if (changed)
                reload();
            searchPane.focusSearch(false);
        } else if (view === "Activity")
            backend.refreshActivity();
        else if (["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(view) >= 0)
            reload();
    }
    // Reload the current view. Without force, a cached snapshot serves
    // instantly with no worker; force always queries native managers.
    function reload(force, preserveSelection) {
        // Do not present matches for an older search as matches for a new one.
        const sameQuery = currentView !== "Search" || searchPane.text.trim() === resultQuery;
        // An empty search does not start a backend query. Invalidate rows
        // left by another page or by a previous, nonempty search.
        const emptySearch = currentView === "Search" && searchPane.text.trim().length === 0;
        const clearSearchResults = emptySearch && (resultView !== "Search" || resultQuery.length > 0);
        retainedItems = currentView === resultView && sameQuery ? items.slice() : [];
        retainingResults = retainedItems.length > 0;
        revealedRows = new Set(retainedItems.map(rowIdentity));
        resultView = clearSearchResults ? "" : currentView;
        if (currentView === "Search")
            resultQuery = searchPane.text.trim();
        results.currentIndex = -1;
        if (!preserveSelection)
            selectedIdentity = null;
        uncheckedPackages = [];
        // Installed filtering is client-side over the loaded rows (see
        // viewItems), so the backend always returns the full installed set
        // and typing never triggers a native query.
        backend.load(currentView, currentView === "Installed" ? "" : searchPane.text, currentView === "Sources" ? "" : checkedCsv(), useSudo, force === true);
        if (!backend.busy)
            retainingResults = false;
    }
    function choose(index) {
        if (retainingResults || index < 0 || index >= viewItems.length)
            return;
        results.currentIndex = index;
        selectedIdentity = rowIdentity(viewItems[index]);
        backend.select(originalIndex(index));
    }
    function restoreSelection() {
        if (!selectedIdentity || retainingResults)
            return;
        for (let i = 0; i < viewItems.length; i++) {
            if (rowIdentity(viewItems[i]) === selectedIdentity) {
                results.currentIndex = i;
                return;
            }
        }
        results.currentIndex = -1;
        if (currentView !== "Search" || !items.some((row) => rowIdentity(row) === selectedIdentity))
            selectedIdentity = null;
    }
    onViewItemsChanged: Qt.callLater(() => root.restoreSelection())
    function propose(action) {
        if (!retainingResults) {
            backend.propose(action, originalIndex(results.currentIndex));
        }
    }
    function focusResultsAfterLoad() {
        // A streaming reply never moves focus away from a user's control.
        // Down or Ctrl+L moves into the result list explicitly.
    }
    Settings {
        id: preferences
        category: "Browser"
        property int appearance: 0
        property bool reduceMotion: false
        property string sourceList: ""
        property string source: ""
        property string authorization: "polkit"
        property int nameWidth: 202
        property int versionWidth: 150
        property string sortColumn: ""
        property bool sortAscending: true
        property bool backgroundMode: false
        property bool autostart: false
    }
    onClosing: function (close) {
        if (!forceQuit && preferences.backgroundMode && trayAvailable) {
            close.accepted = false;
            root.hide();
            return;
        }
        if (backend.busy) {
            close.accepted = false;
            closePending = true;
            backend.cancel();
        }
    }
    Connections {
        target: backend
        function onDetailsChanged() {
            if (root.motionEnabled && root.selected !== null && backend.details !== "{}")
                detailsReveal.restart();
        }
        function onBusyChanged() {
            if (!backend.busy) {
                // The preview or an error has arrived. A queued opening can
                // briefly transition through idle before its worker starts.
                Qt.callLater(() => { if (!backend.busy) root.openingInput = false; });
                root.retainingResults = false;
                root.markChangedRows();
                if (!backend.writing && !postWriteReload.running)
                    root.beforeWrite = null;
            }
            if (!backend.busy && root.closePending)
                root.close();
            else if (!backend.busy && root.selected !== null)
                root.focusResultsAfterLoad();
        }
        function onWritingChanged() {
            if (backend.writing) {
                const snapshot = {};
                for (const row of root.liveItems)
                    snapshot[root.rowIdentity(row)] = root.packageState(row);
                root.beforeWrite = snapshot;
                root.completedRows = [];
            }
            if (!backend.writing && ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0)
                postWriteReload.restart();
        }
        function onRowsChanged() {
            if (root.liveItems.length > 0) {
                root.retainingResults = false;
                if (!backend.writing)
                    root.markChangedRows();
            }
            // Sorting and streaming may reorder rows. Re-select by identity
            // without firing another backend selection or moving focus.
            Qt.callLater(root.restoreSelection);
        }
        function onConfirmationChanged() {
            if (backend.confirmation.length) {
                root.openingInput = false;
                if (!confirmation.opened)
                    root.rememberDialogFocus();
                confirmation.open();
            } else
                confirmation.close();
        }
    }
    Timer {
        interval: 30000
        running: preferences.backgroundMode
        onTriggered: root.checkUpdates(false)
    }
    Timer {
        interval: 300000
        repeat: true
        running: preferences.backgroundMode
        onTriggered: root.checkUpdates(false)
    }
    Timer {
        id: completionHold
        interval: 1000
        onTriggered: root.completedRows = []
    }
    NumberAnimation {
        id: detailsReveal
        target: detailsPanel.detailsContentItem
        property: "opacity"
        from: 0.55
        to: 1
        duration: root.revealDuration
        easing.type: Easing.OutCubic
    }
    Timer {
        interval: backend.busy ? 40 : 200
        running: true
        repeat: true
        onTriggered: backend.poll()
    }
    Timer {
        id: postWriteReload
        interval: 0
        onTriggered: {
            if (!backend.busy && !root.closePending)
                root.reload(true, true);
        }
    }
    Component.onCompleted: {
        // Explicit --from flags seed the session checklist without
        // persisting; otherwise restore the stored list, migrating the
        // legacy single-source preference on first sight.
        const cli = collectArguments("--from").filter((id) => sourceIds.indexOf(id) >= 0);
        if (cli.length > 0)
            sourceSelection = cli.join(",");
        else if (preferences.sourceList !== "")
            sourceSelection = preferences.sourceList;
        else if (preferences.source !== "")
            sourceSelection = preferences.source;
        // Restore the persisted column layout; stored values predate
        // validation, so clamp widths and allowlist the sort column.
        if (preferences.nameWidth >= 80 && preferences.nameWidth <= 600)
            nameWidth = preferences.nameWidth;
        if (preferences.versionWidth >= 80 && preferences.versionWidth <= 600)
            versionWidth = preferences.versionWidth;
        if (["name", "version", "status", "summary", "capabilities"].indexOf(preferences.sortColumn) >= 0)
            sortColumn = preferences.sortColumn;
        sortAscending = preferences.sortAscending;
        reload();
        Qt.callLater(() => {
            if (root.visible && root.currentView === "Search")
                searchPane.focusSearch(false);
        });
        const opening = Qt.application.arguments.slice(1).filter((argument) => argument.startsWith("file://") || argument.startsWith("https://") || argument.startsWith("flatpak+https://") || argument.startsWith("/"));
        if (opening.length === 1)
            Qt.callLater(() => root.openExternalInput(opening[0]));
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 196
            visible: !root.navigationCollapsed
            color: root.surface
            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 16
                spacing: 8
                RowLayout {
                    spacing: 10
                    Layout.topMargin: 12
                    Image {
                        objectName: "appLogo"
                        source: root.logoIconSource
                        sourceSize.width: 30
                        sourceSize.height: 30
                        fillMode: Image.PreserveAspectFit
                        Accessible.ignored: true
                    }
                    Controls.Label {
                        text: "PkgDeck"
                        font.pointSize: root.font.pointSize * 1.8
                        font.bold: true
                        color: root.ink
                    }
                }
                Item { Layout.preferredHeight: 24 }
                Repeater {
                    model: ["Search", "Installed", "Updates", "Clean", "Sources", "Settings"]
                    delegate: ActionButton {
                        required property string modelData
                        Layout.fillWidth: true
                        text: modelData
                        symbol: ({"Search":"search", "Installed":"installed", "Updates":"updates", "Clean":"remove", "Sources":"sources", "Settings":"settings"})[modelData]
                        navigation: true
                        primary: root.currentView === modelData
                        onClicked: root.openView(modelData)
                    }
                }
                Item { Layout.fillHeight: true }
                RowLayout {
                    objectName: "signatureFooter"
                    Layout.fillWidth: true
                    spacing: 4
                    Controls.Label { objectName: "signaturePrefix"; text: "Made with"; color: root.muted; font.pointSize: root.font.pointSize * 0.9 }
                    DeckIcon { name: "heart"; ink: "#e34b5f"; Layout.preferredWidth: 14; Layout.preferredHeight: 14 }
                    Controls.Label { objectName: "signatureAuthor"; text: "by astro"; color: root.muted; font.pointSize: root.font.pointSize * 0.9 }
                }
            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.alignment: Qt.AlignTop
            Layout.margins: root.compact ? 12 : 28
            spacing: root.compact ? 10 : 14
            RowLayout {
                Layout.fillWidth: true
                ThemedComboBox {
                    objectName: "navigationView"
                    visible: root.navigationCollapsed
                    model: ["Search", "Installed", "Updates", "Clean", "Sources", "Activity", "Settings"]
                    currentIndex: model.indexOf(root.currentView)
                    onActivated: root.openView(currentText)
                    Accessible.name: "Navigation"
                    Layout.fillWidth: true
                }
                Kirigami.Heading {
                    visible: !root.navigationCollapsed
                    text: root.currentView
                    color: root.ink
                    level: 1
                    font.pointSize: root.font.pointSize * 1.6
                    font.bold: true
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "addPackageButton"
                    text: "Add…"
                    symbol: "package"
                    visible: root.currentView === "Search" || root.currentView === "Sources"
                    enabled: !backend.writing
                    onClicked: { root.rememberDialogFocus(); addPackageDialog.open(); }
                    Accessible.name: "Add from file or link"
                }
                ActionButton {
                    objectName: "activityIndicator"
                    visible: !root.navigationCollapsed && root.currentView !== "Activity"
                    text: root.queuedCount > 0 ? "Activity · " + root.queuedCount : backend.writing ? "Working" : "Activity"
                    symbol: "updates"
                    Accessible.name: root.queuedCount > 0 ? "Activity, " + root.queuedCount + " queued" : backend.writing ? "Activity, working" : "Activity"
                    onClicked: root.openView("Activity")
                }
                ActionButton {
                    objectName: "sourceFilter"
                    id: sourceFilterButton
                    visible: !root.navigationCollapsed && ["Search", "Installed", "Updates", "Clean"].indexOf(root.currentView) >= 0
                    text: root.viewSourceFilters[root.currentView] ? root.sourceSummary() : "Filter sources"
                    symbol: "sources"
                    onClicked: root.toggleSourcePopup()
                    Accessible.name: "Filter this page by package source"
                    Layout.preferredWidth: 160
                    Controls.Popup {
                        id: sourcePopup
                        objectName: "sourcePopup"
                        parent: Controls.Overlay.overlay
                        property var draftSources: []
                        property string searchText: ""
                        property bool showUnavailable: false
                        x: root.width - width - (root.navigationCollapsed ? 12 : 28)
                        y: root.navigationCollapsed ? 106 : 76
                        width: Math.min(340, root.width - 32)
                        height: Math.min(460, root.height - 100, implicitHeight)
                        padding: 10
                        modal: true
                        Controls.Overlay.modal: Rectangle { color: "transparent" }
                        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
                        onOpened: {
                            searchText = "";
                            pickerSearch.text = "";
                            draftSources = root.effectiveSources().filter((id) => root.sourceInfo(id).availability_kind === "available" && root.sourceSupportsView(id));
                            Qt.callLater(() => { if (sourcePopup.visible) pickerSearch.forceActiveFocus(); });
                        }
                        onClosed: root.restoreDialogFocus()
                        contentItem: ColumnLayout {
                            spacing: 8
                            Controls.Label {
                                visible: root.sourceCatalog.length === 0
                                text: "Checking sources…"
                                color: root.muted
                            }
                            Controls.TextField {
                                id: pickerSearch
                                objectName: "sourcePickerSearch"
                                Layout.fillWidth: true
                                placeholderText: "Find a source"
                                Accessible.name: "Find a source"
                                onTextChanged: sourcePopup.searchText = text
                            }
                            Flickable {
                                id: sourceList
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                implicitHeight: checklist.height
                                clip: true
                                contentWidth: width
                                contentHeight: checklist.height
                                Column {
                                    id: checklist
                                    width: sourceList.width
                                    Repeater {
                                        id: checklistRepeater
                                        model: root.pickerItems()
                                        delegate: Column {
                                            required property string modelData
                                            required property int index
                                            readonly property string sourceId: modelData
                                            property alias checkBox: checkRow
                                            width: checklist.width
                                            readonly property bool usable: root.sourceInfo(sourceId).availability_kind === "available" && root.sourceSupportsView(sourceId)
                                            readonly property string section: usable ? root.sourceCategory(sourceId) : "Unavailable"
                                            Controls.Label {
                                                width: parent.width
                                                topPadding: 8
                                                text: parent.section
                                                font.bold: true
                                                color: root.muted
                                                visible: index === 0 || root.pickerItems()[index - 1] === undefined ||
                                                    (parent.usable ? root.sourceCategory(root.pickerItems()[index - 1]) : "Unavailable") !== parent.section
                                            }
                                            Controls.CheckDelegate {
                                                id: checkRow
                                                objectName: "sourceCheck-" + modelData
                                                width: parent.width
                                                text: root.sourceDisplayName(modelData)
                                                checked: sourcePopup.draftSources.indexOf(modelData) >= 0
                                                enabled: parent.usable && root.checkedSources().indexOf(modelData) >= 0 && (!checked || sourcePopup.draftSources.length > 1)
                                                onToggled: root.toggleDraftSource(modelData)
                                                indicator: TickBox {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    anchors.left: parent.left
                                                    anchors.leftMargin: 8
                                                    ticked: checkRow.checked
                                                }
                                                background: Rectangle { color: checkRow.hovered ? root.selection : "transparent" }
                                                contentItem: Text {
                                                    text: checkRow.text
                                                    color: checkRow.enabled ? root.ink : root.muted
                                                    elide: Text.ElideRight
                                                    verticalAlignment: Text.AlignVCenter
                                                    leftPadding: 34
                                                }
                                            }
                                            Controls.Label {
                                                width: parent.width - 34
                                                x: 34
                                                text: !root.sourceSupportsView(modelData) ? "Not supported in this view" : (root.sourceInfo(modelData).summary || "Unavailable")
                                                color: root.muted
                                                elide: Text.ElideRight
                                                visible: !parent.usable
                                            }
                                        }
                                    }
                                }
                                Controls.ScrollBar.vertical: Controls.ScrollBar { }
                            }
                            ActionButton {
                                objectName: "unavailableSourceToggle"
                                Layout.fillWidth: true
                                text: sourcePopup.showUnavailable ? "Hide unavailable" : "Show unavailable"
                                symbol: "help"
                                onClicked: sourcePopup.showUnavailable = !sourcePopup.showUnavailable
                            }
                            RowLayout {
                                Layout.fillWidth: true
                                Item { Layout.fillWidth: true }
                                ActionButton {
                                    text: "Reset"
                                    symbol: "refresh"
                                    onClicked: sourcePopup.draftSources = root.checkedSources().filter((id) => root.sourceInfo(id).availability_kind === "available" && root.sourceSupportsView(id))
                                }
                                ActionButton {
                                    objectName: "applySourceFilter"
                                    text: "Apply"
                                    primary: true
                                    enabled: sourcePopup.draftSources.length > 0
                                    onClicked: root.applySourceDraft()
                                }
                            }
                        }
                        background: Rectangle {
                            color: root.surface
                            radius: 8
                            border.color: root.line
                        }
                    }
                }
            }
            RowLayout {
                visible: root.navigationCollapsed && root.currentView !== "Activity"
                Layout.fillWidth: true
                spacing: 8
                ActionButton {
                    objectName: "compactActivityIndicator"
                    text: root.queuedCount > 0 ? "Activity · " + root.queuedCount : backend.writing ? "Working" : "Activity"
                    symbol: "updates"
                    Layout.fillWidth: true
                    onClicked: root.openView("Activity")
                }
                ActionButton {
                    objectName: "compactSourceFilter"
                    visible: ["Search", "Installed", "Updates", "Clean"].indexOf(root.currentView) >= 0
                    text: root.viewSourceFilters[root.currentView] ? root.sourceSummary() : "Filter sources"
                    symbol: "sources"
                    Layout.fillWidth: true
                    onClicked: root.toggleSourcePopup()
                    Accessible.name: "Filter this page by package source"
                }
            }
            RowLayout {
                visible: root.currentView === "Search"
                Layout.fillWidth: true
                spacing: 8
                SearchPane {
                    id: searchPane
                    Layout.fillWidth: true
                    writing: false
                    surface: root.surface
                    ink: root.ink
                    muted: root.muted
                    line: root.line
                    accent: root.accent
                    onAccent: root.palette.highlightedText
                    textFont: root.font
                    onSubmitted: {
                        root.currentView = "Search";
                        root.queryDirty = false;
                        root.sortColumn = "";
                        root.reload();
                    }
                    onDownRequested: {
                        results.forceActiveFocus();
                        if (root.viewItems.length > 0)
                            root.choose(0);
                    }
                    onQueryEdited: root.queryDirty = true
                }
            }
            RowLayout {
                objectName: "openingNotice"
                visible: root.openingInput
                Layout.fillWidth: true
                spacing: 10
                Controls.BusyIndicator {
                    running: root.openingInput && root.motionEnabled
                    visible: root.motionEnabled
                    Layout.preferredWidth: 20
                    Layout.preferredHeight: 20
                }
                Controls.Label {
                    text: "Opening…"
                    color: root.muted
                    Layout.fillWidth: true
                    Accessible.name: text
                }
                ActionButton {
                    text: "Cancel"
                    symbol: "cancel"
                    onClicked: backend.cancel()
                }
            }
            GridLayout {
                columns: root.compact ? 2 : 3
                columnSpacing: 8
                rowSpacing: 8
                Layout.fillWidth: true
                visible: root.currentView === "Installed"
                Controls.TextField {
                    id: installedFilterField
                    objectName: "installedFilterField"
                    Layout.fillWidth: true
                    Layout.columnSpan: root.compact ? 2 : 1
                    text: root.installedFilter
                    placeholderText: "Filter installed packages"
                    Accessible.name: "Filter installed packages"
                    enabled: true
                    selectByMouse: true
                    implicitHeight: Math.max(44, root.font.pointSize * 3.4)
                    color: root.ink
                    placeholderTextColor: root.muted
                    leftPadding: 14
                    background: Rectangle {
                        color: root.surface
                        radius: 8
                        border.color: installedFilterField.activeFocus ? root.accent : root.line
                        border.width: installedFilterField.activeFocus ? 2 : 1
                    }
                    onTextChanged: root.installedFilter = text
                    onAccepted: {
                        results.forceActiveFocus();
                        if (root.viewItems.length > 0)
                            root.choose(0);
                    }
                    Keys.onDownPressed: {
                        results.forceActiveFocus();
                        if (root.viewItems.length > 0)
                            root.choose(0);
                    }
                }
                Controls.CheckBox {
                    id: multiSourceCheck
                    objectName: "multiSourceCheck"
                    visible: root.items.some(row => row.kind === "package") || root.multiSourceOnly || backend.busy
                    text: "Duplicate installs"
                    checked: root.multiSourceOnly
                    enabled: true
                    onToggled: root.multiSourceOnly = checked
                    Accessible.name: "Show only packages installed from multiple sources"
                    Layout.alignment: Qt.AlignVCenter
                    Layout.fillWidth: root.compact
                    indicator: TickBox {
                        x: 0
                        y: (multiSourceCheck.height - height) / 2
                        ticked: multiSourceCheck.checked
                    }
                    contentItem: Text {
                        text: multiSourceCheck.text
                        color: root.ink
                        verticalAlignment: Text.AlignVCenter
                        leftPadding: 28
                    }
                }
            }
            Controls.ScrollView {
                id: settingsScroll
                objectName: "settingsScroll"
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                Layout.fillHeight: true
                contentWidth: availableWidth
                clip: true
                ColumnLayout {
                    width: settingsScroll.availableWidth
                    spacing: 10
                    Controls.Label { text: "Appearance"; color: root.ink; font.bold: true }
                    ThemedComboBox {
                        objectName: "appearanceSetting"
                        model: ["System", "Dark", "Light"]
                        currentIndex: preferences.appearance
                        onActivated: preferences.appearance = currentIndex
                        Accessible.name: "Appearance"
                        Layout.fillWidth: true
                        Layout.maximumWidth: 420
                    }
                    SettingCheckBox {
                        objectName: "animationsSetting"
                        text: "Animations"
                        checked: !root.reduceMotion
                        onToggled: root.reduceMotion = !checked
                        Accessible.name: "Enable interface animations"
                    }
                    SettingCheckBox {
                        objectName: "backgroundModeSetting"
                        text: "Background checks"
                        checked: preferences.backgroundMode
                        onClicked: {
                            if (!checked && preferences.autostart && !backend.setAutostart(false)) {
                                checked = true;
                                return;
                            }
                            preferences.backgroundMode = checked;
                            if (!checked)
                                preferences.autostart = false;
                        }
                        Accessible.name: text
                    }
                    SettingCheckBox {
                        objectName: "autostartSetting"
                        text: "Start in background at login"
                        checked: preferences.autostart
                        enabled: preferences.backgroundMode && root.trayAvailable
                        onClicked: {
                            if (backend.setAutostart(checked))
                                preferences.autostart = checked;
                            else
                                checked = preferences.autostart;
                        }
                        Accessible.name: text
                    }
                    Controls.Label {
                        text: "Authentication"
                        color: root.ink
                        font.bold: true
                    }
                    ThemedComboBox {
                        objectName: "authorizationSetting"
                        model: ["System prompt", "Existing sudo session"]
                        currentIndex: root.useSudo ? 1 : 0
                        onActivated: {
                            root.useSudo = currentIndex === 1;
                            preferences.authorization = root.useSudo ? "sudo" : "polkit";
                        }
                        Accessible.name: "Authentication"
                        Layout.fillWidth: true
                        Layout.maximumWidth: 420
                    }
                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 1
                        Layout.topMargin: 16
                        color: root.line
                    }
                    Controls.Label {
                        text: "About"
                        color: root.ink
                        font.bold: true
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Controls.Label {
                            objectName: "aboutText"
                            text: "PkgDeck " + backend.version
                            color: root.ink
                            font.bold: true
                            font.pointSize: root.font.pointSize * 1.3
                            Layout.fillWidth: true
                        }
                        ActionButton {
                            objectName: "repositoryLink"
                            text: "GitHub"
                            symbol: ""
                            iconSource: root.repositoryIconSource
                            Accessible.name: "Open PkgDeck on GitHub"
                            onClicked: Qt.openUrlExternally(root.repositoryUrl)
                        }
                    }
                    Controls.Label {
                        text: "Keyboard shortcuts"
                        color: root.ink
                        font.bold: true
                        Layout.topMargin: 12
                    }
                    GridLayout {
                        columns: root.compact ? 1 : 2
                        columnSpacing: 28
                        rowSpacing: 8
                        Layout.fillWidth: true
                        Repeater {
                            objectName: "aboutShortcuts"
                            model: [
                            {action: "Search", keys: "Ctrl+1"},
                            {action: "Installed", keys: "Ctrl+2"},
                            {action: "Updates", keys: "Ctrl+3"},
                            {action: "Clean", keys: "Ctrl+4"},
                            {action: "Sources", keys: "Ctrl+5"},
                            {action: "Search or filter", keys: "Ctrl+F"},
                            {action: "Focus results", keys: "Ctrl+L"},
                            {action: "Select result", keys: "↑ / ↓"},
                            {action: "Install", keys: "Ctrl+I"},
                            {action: "Remove", keys: "Ctrl+D"},
                            {action: "Update", keys: "Ctrl+U"},
                            {action: "Update selected", keys: "Ctrl+Shift+U"},
                            {action: "Refresh source", keys: "Ctrl+M"},
                            {action: "Reload", keys: "Ctrl+R"},
                            {action: "Cancel work", keys: "Esc"}
                            ]
                            delegate: RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                Layout.minimumWidth: root.compact ? 0 : 260
                                spacing: 12
                                Controls.Label {
                                    text: modelData.action
                                    color: root.muted
                                    Layout.fillWidth: true
                                }
                                Controls.Label {
                                    text: modelData.keys
                                    color: root.ink
                                }
                            }
                        }
                    }
                    RowLayout {
                        objectName: "compactSignature"
                        visible: root.navigationCollapsed
                        Layout.topMargin: 12
                        spacing: 4
                        Controls.Label { text: "Made with"; color: root.muted }
                        DeckIcon { name: "heart"; ink: "#e34b5f"; Layout.preferredWidth: 14; Layout.preferredHeight: 14 }
                        Controls.Label { text: "by astro"; color: root.muted }
                    }
                }
            }
            RowLayout {
                visible: root.readFailures.length > 0 && ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                DeckIcon { name: "warning"; ink: root.accent; Layout.preferredWidth: 20; Layout.preferredHeight: 20 }
                Controls.Label {
                    objectName: "sourceFailureNotice"
                    text: root.readFailures.length + (root.readFailures.length === 1 ? " source needs attention" : " sources need attention")
                    color: root.muted
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "sourceFailureDetails"
                    text: "Review"
                    symbol: "help"
                    onClicked: { root.rememberDialogFocus(); sourceFailuresDialog.open(); }
                }
            }
            ActivityPane {
                visible: root.currentView === "Activity"
                entries: root.activityRows
                surface: root.surface
                ink: root.ink
                muted: root.muted
                line: root.line
                accent: root.accent
                textFont: root.font
                onCancelQueued: backend.cancelQueued()
            }
            Rectangle {
                id: resultsBox
                objectName: "resultsBox"
                Layout.fillWidth: true
                Layout.fillHeight: (backend.busy && !root.openingInput) || root.viewItems.length > root.shortListLimit
                Layout.preferredHeight: root.viewItems.length === 0 && !backend.busy ? 150
                    : Math.min(root.shortResultsHeight(), detailsPanel.visible ? root.height * (root.compact ? 0.24 : 0.42) : root.height * 0.7)
                Layout.minimumHeight: root.compact && detailsPanel.visible ? 100 : 130
                visible: root.currentView === root.resultView && root.currentView !== "Settings" && root.currentView !== "Activity" &&
                    (root.viewItems.length > 0 || root.readFailures.length === 0 || backend.busy) &&
                    (root.currentView !== "Search" || root.viewItems.length > 0 || (backend.busy && !root.openingInput) || searchPane.text.trim().length > 0)
                color: root.surface
                radius: 10
                border.color: results.activeFocus ? root.accent : root.line
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 1
                    spacing: 0
                    RowLayout {
                        visible: results.count > 0 || backend.busy || backend.writing || (root.currentView === "Sources" && root.items.some(row => row.kind === "source" && !row.available))
                        Layout.fillWidth: true
                        Layout.margins: 14
                        Controls.Label {
                            objectName: "resultsHeading"
                            text: root.resultsHeading()
                            color: root.muted
                            font.pointSize: root.font.pointSize * 0.9
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                        }
                        Controls.BusyIndicator {
                            objectName: "resultsBusy"
                            running: backend.busy && root.motionEnabled
                            visible: running && results.count > 0
                            Layout.preferredWidth: 18
                            Layout.preferredHeight: 18
                        }
                        ActionButton {
                            objectName: "unavailableSourcesButton"
                            visible: root.currentView === "Sources" && root.items.some(row => row.kind === "source" && !row.available)
                            text: root.showUnavailableSources ? "Hide unavailable" : "Show unavailable (" + root.items.filter(row => row.kind === "source" && !row.available).length + ")"
                            symbol: ""
                            implicitHeight: 30
                            horizontalPadding: 10
                            onClicked: root.showUnavailableSources = !root.showUnavailableSources
                        }
                        Controls.Label {
                            text: "Working…"
                            visible: backend.busy && !root.motionEnabled
                            color: root.muted
                            font.pointSize: root.font.pointSize * 0.9
                        }
                        ActionButton {
                            objectName: "resultsCancel"
                            text: "Cancel"
                            symbol: "cancel"
                            visible: backend.busy
                            implicitHeight: 28
                            onClicked: backend.cancel()
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: root.line; visible: results.count > 0 }
                    RowLayout {
                        visible: !root.compact && results.count > 0
                        spacing: 14
                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.topMargin: 10
                        Layout.bottomMargin: 10
                        Item { visible: root.currentView === "Updates"; Layout.preferredWidth: 28 }
                        Controls.Label {
                            objectName: "columnHeader0"
                            text: (root.currentView === "Sources" ? "SOURCE" : "NAME / SOURCE") + root.sortArrow("name")
                            color: root.muted
                            font.pointSize: root.font.pointSize * 0.9
                            font.underline: sortNameArea.activeFocus
                            elide: Text.ElideRight
                            Layout.preferredWidth: root.nameWidth
                            Layout.fillWidth: root.currentView === "Sources"
                            MouseArea {
                                id: sortNameArea
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                activeFocusOnTab: true
                                Accessible.role: Accessible.Button
                                Accessible.name: "Sort by name"
                                onClicked: root.cycleSort("name")
                                Keys.onSpacePressed: root.cycleSort("name")
                                Keys.onReturnPressed: root.cycleSort("name")
                                MouseArea {
                                    objectName: "columnResize0"
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                    width: 12
                                    cursorShape: Qt.SplitHCursor
                                    property real pressX: 0
                                    property int startWidth: 0
                                    onPressed: (mouse) => { pressX = mouse.x; startWidth = root.nameWidth; }
                                    onPositionChanged: (mouse) => { if (pressed) root.nameWidth = Math.max(80, Math.min(600, Math.round(startWidth + mouse.x - pressX))); }
                                }
                            }
                        }
                        Controls.Label {
                            objectName: "columnHeader1"
                            visible: root.currentView !== "Sources" || root.showUnavailableSources
                            text: (root.currentView === "Sources" ? "STATUS" : root.currentView === "Clean" ? "TYPE" : "VERSION") + root.sortArrow(root.currentView === "Sources" ? "status" : "version")
                            color: root.muted
                            font.pointSize: root.font.pointSize * 0.9
                            font.underline: sortVersionArea.activeFocus
                            elide: Text.ElideRight
                            Layout.preferredWidth: root.versionWidth
                            MouseArea {
                                id: sortVersionArea
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                activeFocusOnTab: true
                                Accessible.role: Accessible.Button
                                Accessible.name: root.currentView === "Sources" ? "Sort by status" : "Sort by version"
                                onClicked: root.cycleSort(root.currentView === "Sources" ? "status" : "version")
                                Keys.onSpacePressed: root.cycleSort(root.currentView === "Sources" ? "status" : "version")
                                Keys.onReturnPressed: root.cycleSort(root.currentView === "Sources" ? "status" : "version")
                                MouseArea {
                                    objectName: "columnResize1"
                                    anchors.right: parent.right
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                    width: 12
                                    cursorShape: Qt.SplitHCursor
                                    property real pressX: 0
                                    property int startWidth: 0
                                    onPressed: (mouse) => { pressX = mouse.x; startWidth = root.versionWidth; }
                                    onPositionChanged: (mouse) => { if (pressed) root.versionWidth = Math.max(80, Math.min(600, Math.round(startWidth + mouse.x - pressX))); }
                                }
                            }
                        }
                        Controls.Label {
                            objectName: "columnHeader2"
                            visible: root.currentView !== "Sources"
                            text: (root.currentView === "Sources" ? "CAPABILITIES" : "SUMMARY") + root.sortArrow(root.currentView === "Sources" ? "capabilities" : "summary")
                            color: root.muted
                            font.pointSize: root.font.pointSize * 0.9
                            font.underline: sortSummaryArea.activeFocus
                            elide: Text.ElideRight
                            Layout.fillWidth: true
                            MouseArea {
                                id: sortSummaryArea
                                anchors.fill: parent
                                cursorShape: Qt.PointingHandCursor
                                activeFocusOnTab: true
                                Accessible.role: Accessible.Button
                                Accessible.name: root.currentView === "Sources" ? "Sort by capabilities" : "Sort by summary"
                                onClicked: root.cycleSort(root.currentView === "Sources" ? "capabilities" : "summary")
                                Keys.onSpacePressed: root.cycleSort(root.currentView === "Sources" ? "capabilities" : "summary")
                                Keys.onReturnPressed: root.cycleSort(root.currentView === "Sources" ? "capabilities" : "summary")
                            }
                        }
                    }
                    ListView {
                        id: results
                        objectName: "packageResults"
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        model: root.viewItems
                        clip: true
                        reuseItems: true
                        enabled: !root.retainingResults
                        opacity: root.retainingResults ? 0.65 : 1
                        Behavior on opacity { NumberAnimation { duration: root.feedbackDuration } }
                        currentIndex: -1
                        onCountChanged: {
                            if (!root.selectedIdentity)
                                currentIndex = -1;
                        }
                        keyNavigationEnabled: false
                        activeFocusOnTab: true
                        Controls.ScrollBar.vertical: Controls.ScrollBar {}
                        Keys.onDownPressed: root.choose(Math.min(count - 1, currentIndex + 1))
                        Keys.onUpPressed: root.choose(Math.max(0, currentIndex - 1))
                        Keys.onPressed: (event) => {
                            if (event.key === Qt.Key_PageDown)
                                root.choose(Math.min(count - 1, (currentIndex < 0 ? 0 : currentIndex) + 10));
                            else if (event.key === Qt.Key_PageUp)
                                root.choose(Math.max(0, (currentIndex < 0 ? 0 : currentIndex) - 10));
                            else if (event.key === Qt.Key_Home)
                                root.choose(0);
                            else if (event.key === Qt.Key_End)
                                root.choose(count - 1);
                            else
                                return;
                            event.accepted = true;
                        }
                        delegate: Controls.ItemDelegate {
                            id: packageRow
                            required property var modelData
                            required property int index
                            function reveal() {
                                rowReveal.complete();
                                opacity = 1;
                                if (root.revealRow(modelData))
                                    rowReveal.restart();
                            }
                            Component.onCompleted: reveal()
                            ListView.onReused: reveal()
                            Connections {
                                target: root
                                function onMotionEnabledChanged() {
                                    if (!root.motionEnabled)
                                        rowReveal.complete();
                                }
                            }
                            NumberAnimation {
                                id: rowReveal
                                target: packageRow
                                property: "opacity"
                                from: 0.65
                                to: 1
                                duration: root.revealDuration
                                easing.type: Easing.OutCubic
                            }
                            width: Math.max(0, ListView.view.width - 16)
                            height: (root.compact ? (modelData.kind === "source" ? Math.max(68, root.font.pointSize * 5.5) : Math.max(94, root.font.pointSize * 8.5)) : Math.max(56, root.font.pointSize * 5)) + (modelData.groupStart ? 38 : 0)
                            topPadding: modelData.groupStart ? 38 : 0
                            leftPadding: 16
                            rightPadding: 16
                            highlighted: results.currentIndex === index
                            enabled: true
                            Accessible.name: (modelData.kind === "package" ? (modelData.update === "available" ? "Update available. " : (root.isInstalled(modelData) ? "Installed. " : "Not installed. ")) : "") + modelData.name + ", " + modelData.source + ", " + (modelData.summary || "")
                            onClicked: { results.forceActiveFocus(); root.choose(index); }
                            Rectangle {
                                visible: !!modelData.groupStart
                                anchors.top: parent.top
                                anchors.left: parent.left
                                anchors.right: parent.right
                                height: 38
                                color: root.selection
                                z: 2
                                Rectangle { width: 3; height: parent.height; color: root.accent }
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 16
                                    anchors.rightMargin: 16
                                    Controls.Label {
                                        objectName: modelData.groupStart ? "packageGroupTitle" : ""
                                        text: modelData.groupTitle || ""
                                        color: root.ink
                                        font.bold: true
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        text: (modelData.groupCount || 0) + " packages · " + (modelData.groupSources || []).join(" + ")
                                        color: root.accent
                                        font.pointSize: root.font.pointSize * 0.9
                                    }
                                }
                            }
                            background: Rectangle {
                                Behavior on color { ColorAnimation { duration: root.feedbackDuration } }
                                color: packageRow.highlighted ? root.selection : (packageRow.hovered ? root.canvas : "transparent")
                                Rectangle {
                                    anchors.fill: parent
                                    color: root.accent
                                    opacity: root.completedRows.indexOf(root.rowIdentity(packageRow.modelData)) >= 0 ? 0.18 : 0
                                    Behavior on opacity { NumberAnimation { duration: root.motionEnabled ? 240 : 0 } }
                                }
                                Rectangle { anchors.fill: parent; color: "transparent"; border.width: 1; border.color: root.accent; visible: packageRow.activeFocus || (results.activeFocus && packageRow.highlighted) }
                                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.line; opacity: 0.5 }
                            }
                            contentItem: RowLayout {
                                spacing: 14
                                Controls.CheckBox {
                                    id: packageCheck
                                    visible: root.currentView === "Updates" && modelData.kind === "package"
                                    checked: root.packageChecked(modelData)
                                    enabled: true
                                    onToggled: root.togglePackage(modelData)
                                    Accessible.name: "Select " + (modelData.name || "")
                                    Layout.preferredWidth: 28
                                    Layout.alignment: Qt.AlignVCenter
                                    indicator: TickBox {
                                        anchors.centerIn: parent
                                        ticked: packageCheck.checked
                                    }
                                    contentItem: Item {}
                                }
                                ColumnLayout {
                                    spacing: 4
                                    Layout.preferredWidth: root.compact ? -1 : root.nameWidth
                                    Layout.fillWidth: root.compact || modelData.kind === "source"
                                    Controls.Label {
                                        objectName: "packageName"
                                        text: modelData.kind === "source" ? root.sourceDisplayName(modelData.source) : (modelData.display_name || modelData.name)
                                        color: root.ink
                                        font.bold: true
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    RowLayout {
                                        visible: modelData.kind !== "source"
                                        Layout.fillWidth: true
                                        spacing: 6
                                        DeckIcon {
                                            objectName: "packageIconFallback"
                                            visible: rowIcon.status !== Image.Ready
                                            name: modelData.source
                                            ink: root.muted
                                            Layout.preferredWidth: 14
                                            Layout.preferredHeight: 14
                                        }
                                        Image {
                                            id: rowIcon
                                            objectName: "packageIcon"
                                            visible: status === Image.Ready
                                            asynchronous: true
                                            source: root.iconUrl(modelData.icon || "")
                                            sourceSize.width: 14
                                            sourceSize.height: 14
                                            fillMode: Image.PreserveAspectFit
                                            Layout.preferredWidth: 14
                                            Layout.preferredHeight: 14
                                            Accessible.ignored: true
                                        }
                                        Controls.Label {
                                        objectName: "packageSourceLine"
                                        text: modelData.source.toUpperCase() + (modelData.remote ? " · " + modelData.remote : "") + ((modelData.source === "flatpak" || root.containerSource(modelData.source)) ? " · " + (modelData.scope === "system" ? "System" : "User") : "")
                                        color: root.muted
                                        font.pointSize: root.font.pointSize * 0.9
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                        }
                                    }
                                    Controls.Label {
                                        objectName: "compactVersion"
                                        visible: root.compact && (modelData.kind !== "source" || root.showUnavailableSources)
                                        text: root.versionText(modelData)
                                        font.family: "monospace"
                                        font.pointSize: root.font.pointSize * 0.9
                                        color: modelData.kind === "failure" ? "#e87979" : (modelData.update === "available" ? root.accent : root.muted)
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        visible: root.compact && modelData.kind !== "source"
                                        text: modelData.summary || ""
                                        color: root.muted
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                }
                                Controls.Label {
                                    objectName: "wideVersion"
                                    visible: !root.compact && (modelData.kind !== "source" || root.showUnavailableSources)
                                    Layout.preferredWidth: root.versionWidth
                                    text: root.versionText(modelData)
                                    font.family: "monospace"
                                    color: modelData.kind === "failure" ? "#e87979" : (modelData.update === "available" ? root.accent : root.muted)
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                    font.pointSize: root.font.pointSize * 0.9
                                }
                                Controls.Label {
                                    visible: !root.compact && modelData.kind !== "source"
                                    Layout.fillWidth: true
                                    text: modelData.summary || ""
                                    color: root.muted
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                }
                                Controls.CheckBox {
                                    id: managerEnabled
                                    objectName: "managerEnabled"
                                    visible: root.currentView === "Sources" && modelData.kind === "source" && modelData.available
                                    text: checked ? "Enabled" : "Disabled"
                                    checked: root.checkedSources().indexOf(modelData.source) >= 0
                                    enabled: !backend.writing && (!checked || root.checkedSources().length > 1)
                                    Accessible.name: (checked ? "Disable " : "Enable ") + root.sourceDisplayName(modelData.source)
                                    Controls.ToolTip.visible: hovered && root.compact
                                    Controls.ToolTip.text: Accessible.name
                                    onClicked: root.setManagerEnabled(modelData.source, checked)
                                    indicator: TickBox {
                                        x: 0
                                        y: (managerEnabled.height - height) / 2
                                        ticked: managerEnabled.checked
                                    }
                                    contentItem: Text {
                                        text: managerEnabled.text
                                        color: root.ink
                                        verticalAlignment: Text.AlignVCenter
                                        leftPadding: 28
                                    }
                                }
                                ActionButton {
                                    objectName: "rowContainerPull"
                                    visible: modelData.kind === "package" && root.containerSource(modelData.source) && root.isInstalled(modelData) && !!modelData.reference
                                    enabled: (!backend.busy || backend.writing) && !root.retainingResults
                                    text: root.compact ? "" : "Pull"
                                    symbol: "updates"
                                    glyphColor: root.accent
                                    Accessible.name: "Pull " + (modelData.display_name || modelData.name) + " from " + modelData.source
                                    tooltipText: Accessible.name
                                    Layout.preferredWidth: root.compact ? 38 : 80
                                    horizontalPadding: 8
                                    onClicked: backend.propose("upgrade", root.originalIndex(index))
                                }
                                ActionButton {
                                    objectName: "rowPackageAction"
                                    visible: (modelData.kind === "package" && (!root.updateOnly(modelData.source) || modelData.update === "available")) || modelData.kind === "cleanup"
                                    enabled: (!backend.busy || backend.writing) && !root.retainingResults
                                    text: root.compact ? "" : (modelData.kind === "cleanup" ? "Clean" : (root.currentView === "Updates" || root.updateOnly(modelData.source) ? "Update" : (root.isInstalled(modelData) ? "Remove" : "Install")))
                                    symbol: modelData.kind === "cleanup" ? "remove" : (root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "updates" : (root.isInstalled(modelData) ? "remove" : "install")
                                    glyphColor: (root.currentView === "Updates" || root.updateOnly(modelData.source)) ? root.accent : root.isInstalled(modelData) ? (root.dark ? "#f18b91" : "#b42332") : (root.dark ? "#77d6a0" : "#187442")
                                    Accessible.name: (modelData.kind === "cleanup" ? "Run cleanup " : ((root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "Update " : (root.isInstalled(modelData) ? "Remove " : "Install "))) + (modelData.display_name || modelData.name) + " from " + modelData.source
                                    tooltipText: Accessible.name
                                    Layout.preferredWidth: root.compact ? 38 : 88
                                    horizontalPadding: 8
                                    onClicked: backend.propose(modelData.kind === "cleanup" ? "clean" : ((root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "upgrade" : (root.isInstalled(modelData) ? "remove" : "install")), root.originalIndex(index))
                                }
                            }
                        }
                        Column {
                            anchors.centerIn: parent
                            width: parent.width - 32
                            spacing: 8
                            visible: results.count === 0
                            DeckIcon {
                                name: "refresh"
                                ink: root.accent
                                visible: backend.busy
                                anchors.horizontalCenter: parent.horizontalCenter
                                width: 28
                                height: 28
                                RotationAnimation on rotation {
                                    running: backend.busy && root.motionEnabled
                                    from: 0
                                    to: 360
                                    duration: 1200
                                    loops: Animation.Infinite
                                }
                            }
                            Controls.Label {
                                objectName: "emptyState"
                                width: parent.width
                                horizontalAlignment: Text.AlignHCenter
                                wrapMode: Text.WordWrap
                                color: root.muted
                                visible: text.length > 0
                                text: root.emptyStateMessage()
                            }
                            ActionButton {
                                objectName: "clearResultFilters"
                                anchors.horizontalCenter: parent.horizontalCenter
                                visible: !backend.busy && root.readFailures.length === 0 &&
                                    (root.viewSourceFilters[root.currentView] !== undefined ||
                                    (root.currentView === "Installed" && (root.installedFilter.length > 0 || root.multiSourceOnly)))
                                text: "Clear filters"
                                symbol: "cancel"
                                onClicked: root.clearVisibleFilters()
                            }
                        }
                    }
                }
            }
            PackageDetails {
                id: detailsPanel
                visible: root.selected !== null && ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0
                    && (root.selected.kind !== "package" || (root.detailMatchesSelection
                        && (root.detailText().length > 0 || root.visibleScreenshots.length > 0 || detailsPanel.metadataText.length > 0)))
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(root.height * (root.compact ? 0.32 : 0.48), detailsPanel.idealHeight)
                selected: root.selected
                selectionIdentity: root.rowIdentity(root.selected)
                screenshots: root.visibleScreenshots
                description: root.detailText()
                detailsData: root.detailMatchesSelection ? root.detail : ({})
                iconSource: root.iconUrl(root.selectedIcon)
                compact: root.compact
                motionEnabled: root.motionEnabled
                detailMatchesSelection: root.detailMatchesSelection
                textFont: root.font
                canvas: root.canvas
                surface: root.surface
                ink: root.ink
                muted: root.muted
                line: root.line
                accent: root.accent
                onCloseRequested: {
                    results.currentIndex = -1;
                    root.selectedIdentity = null;
                    results.forceActiveFocus();
                }
                onScreenshotRequested: (url, caption) => {
                    root.rememberDialogFocus();
                    root.screenshotUrl = url;
                    root.screenshotCaption = caption;
                    screenshotDialog.open();
                }
                onScreenshotFailed: (url, identity) => root.hideFailedScreenshot(url, identity)
            }
            Flow {
                objectName: "updatesActions"
                Layout.fillWidth: true
                spacing: 8
                visible: ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0
                ActionButton {
                    objectName: "cleanAllButton"
                    visible: root.currentView === "Clean" && root.items.filter((row) => row.kind === "cleanup").length > 1
                    text: "Clean all"
                    symbol: "remove"
                    primary: true
                    enabled: !backend.busy || backend.writing
                    onClicked: backend.propose("clean-all", -1)
                }
                UpdatesActions {
                    active: root.currentView === "Updates"
                    width: Math.min(parent.width, preferredWidth)
                    compact: root.compact
                    busy: backend.busy && !backend.writing
                    writing: false
                    upgradable: backend.upgradable
                    selectedCount: root.selectedCount()
                    uncheckedCount: root.uncheckedPackages.length
                    failed: root.items.some((row) => row.kind === "failure")
                    textFont: root.font
                    surface: root.surface
                    ink: root.ink
                    muted: root.muted
                    line: root.line
                    accent: root.accent
                    onAccent: root.palette.highlightedText
                    selection: root.selection
                    onUpgradeRequested: root.upgradeUpdates()
                    onSelectNoneRequested: root.selectNonePackages()
                    onSelectAllRequested: root.uncheckedPackages = []
                }
                ActionButton {
                    objectName: "repositoriesButton"
                    visible: root.currentView === "Sources"
                    text: "Repositories"
                    symbol: "sources"
                    enabled: !backend.busy
                    onClicked: { root.rememberDialogFocus(); repositoriesDialog.open(); }
                }
                ActionButton {
                    objectName: "refreshButton"
                    visible: root.selected !== null && root.selected.kind === "source"
                    text: "Refresh sources"
                    symbol: "refresh"
                    primary: true
                    enabled: (!backend.busy || backend.writing) && root.selected !== null && root.selected.kind === "source" && root.selected.available
                    onClicked: root.propose("refresh")
                }
                ActionButton {
                    objectName: "reloadButton"
                    visible: root.currentView !== "Search" || searchPane.text.trim().length > 0 || root.viewItems.length > 0
                    text: root.compact ? "" : "Reload"
                    symbol: "refresh"
                    Accessible.name: "Reload"
                    tooltipText: root.compact ? "Reload" : ""
                    enabled: !backend.busy || backend.writing
                    onClicked: root.reload(true)
                }
            }
            Item {
                visible: ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0 && !backend.busy &&
                    root.viewItems.length <= root.shortListLimit
                Layout.fillHeight: true
            }
        }
    }
    ThemedDialog {
        id: repositoriesDialog
        objectName: "repositoriesDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 850)
        height: Math.min(root.height - 40, 640, Math.max(260, 160 + Math.min((root.repositoryReport.repositories || []).length, 5) * (width < 620 ? 110 : 76)))
        title: "Repositories"
        modal: true
        standardButtons: Controls.Dialog.Close
        onOpened: backend.loadRepositories()
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 12
            Flow {
                Layout.fillWidth: true
                spacing: 8
                ActionButton {
                    text: "Add Flatpak repository"; symbol: "install"
                    enabled: !backend.busy
                    onClicked: { root.rememberDialogFocus(); addRepositoryDialog.open(); }
                }
                ActionButton {
                    text: "Software Sources"; symbol: "settings"
                    enabled: !backend.busy
                    onClicked: root.repositoryChange({backend: "apt", name: "sources", scope: "system"}, "open_editor")
                }
                ActionButton { text: ""; symbol: "refresh"; Accessible.name: "Reload repositories"; tooltipText: Accessible.name; enabled: !backend.busy; onClicked: backend.loadRepositories() }
            }
            Controls.BusyIndicator { visible: backend.busy; running: visible; Layout.alignment: Qt.AlignHCenter }
            ListView {
                objectName: "repositoryList"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 2
                model: root.repositoryReport.repositories || []
                Controls.ScrollBar.vertical: Controls.ScrollBar {}
                delegate: Rectangle {
                    required property var modelData
                    width: ListView.view.width - 14
                    height: repositoryCard.implicitHeight + 12
                    color: root.surface
                    radius: 6
                    GridLayout {
                        id: repositoryCard
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 6
                        columns: repositoriesDialog.width < 620 ? 1 : 2
                        columnSpacing: 12
                        rowSpacing: 4
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            Controls.CheckBox {
                                objectName: "repositoryEnabled"
                                checked: modelData.enabled
                                enabled: !backend.busy && (modelData.backend === "flatpak" || modelData.backend === "fwupd")
                                Accessible.name: "Enable " + (modelData.title || modelData.name)
                                onClicked: {
                                    root.repositoryChange(modelData, "set_enabled", {enabled: checked});
                                    checked = Qt.binding(() => modelData.enabled);
                                }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                Controls.Label {
                                    objectName: "repositoryTitle"
                                    text: modelData.title || modelData.name
                                    textFormat: Text.PlainText
                                    color: root.ink
                                    elide: Text.ElideRight
                                    Layout.fillWidth: true
                                }
                                Controls.Label {
                                    text: modelData.backend.toUpperCase() + " · " + (modelData.scope === "system" ? "System" : "User")
                                    color: root.muted
                                    font.pointSize: root.font.pointSize * 0.9
                                }
                                Controls.Label {
                                    objectName: "repositoryUrlLabel"
                                    visible: !!modelData.url && (modelData.title || modelData.name).indexOf(modelData.url) < 0
                                    text: modelData.url || ""
                                    textFormat: Text.PlainText
                                    color: root.muted
                                    font.pointSize: root.font.pointSize * 0.9
                                    elide: Text.ElideMiddle
                                    Layout.fillWidth: true
                                }
                            }
                        }
                        RowLayout {
                            Layout.alignment: Qt.AlignVCenter | Qt.AlignRight
                            Controls.Label {
                                visible: modelData.priority !== null && modelData.priority !== undefined
                                text: "Priority " + modelData.priority
                                color: root.muted
                                font.pointSize: root.font.pointSize * 0.9
                            }
                            ActionButton {
                                text: ""; symbol: "up"; visible: modelData.backend === "flatpak"
                                Accessible.name: "Increase repository priority"; enabled: !backend.busy && modelData.priority < 9999
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority + 1})
                            }
                            ActionButton {
                                text: ""; symbol: "down"; visible: modelData.backend === "flatpak"
                                Accessible.name: "Decrease repository priority"; enabled: !backend.busy && modelData.priority > 0
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority - 1})
                            }
                            ActionButton {
                                text: ""; symbol: "settings"; visible: modelData.backend === "apt"
                                Accessible.name: "Edit software sources"; enabled: !backend.busy
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange({backend: "apt", name: "sources", scope: "system"}, "open_editor")
                            }
                            ActionButton {
                                objectName: "removeRepositoryButton"
                                text: ""; symbol: "remove"; visible: modelData.backend === "flatpak"
                                Accessible.name: "Remove " + modelData.name; enabled: !backend.busy
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "remove")
                            }
                        }
                    }
                }
            }
            Controls.Label { Layout.fillWidth: true; color: root.muted; textFormat: Text.PlainText; wrapMode: Text.WordWrap; text: (root.repositoryReport.errors || []).join("\n"); visible: text.length > 0 }
            Controls.Label {
                objectName: "repositoryStatus"
                Layout.fillWidth: true
                color: root.muted
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                text: backend.status
                visible: text.length > 0 && text !== "Ready" && text !== "Repositories loaded."
            }
        }
    }
    ThemedDialog {
        id: addRepositoryDialog
        objectName: "addRepositoryDialog"
        readonly property bool validName: /^[A-Za-z0-9._][A-Za-z0-9._-]*$/.test(repositoryName.text.trim())
        readonly property bool validUrl: /^https:\/\/\S+\.flatpakrepo$/.test(repositoryUrl.text.trim())
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 48, 560)
        title: "Add Flatpak repository"
        modal: true
        standardButtons: Controls.Dialog.Ok | Controls.Dialog.Cancel
        onOpened: {
            repositoryName.text = "";
            repositoryUrl.text = "";
            standardButton(Controls.Dialog.Ok).enabled = Qt.binding(() => addRepositoryDialog.validName && addRepositoryDialog.validUrl);
            repositoryName.forceActiveFocus();
        }
        onClosed: root.restoreDialogFocus()
        onAccepted: root.repositoryChange({backend: "flatpak", name: repositoryName.text.trim(), scope: repositoryScope.currentIndex === 0 ? "user" : "system"}, "add", {url: repositoryUrl.text.trim()})
        contentItem: ColumnLayout {
            Controls.TextField { id: repositoryName; objectName: "repositoryName"; placeholderText: "Name"; Accessible.name: "Repository name"; Layout.fillWidth: true }
            Controls.Label {
                objectName: "repositoryNameError"
                text: "Use letters, numbers, . _ - (no leading -)."
                visible: repositoryName.text.length > 0 && !addRepositoryDialog.validName
                color: root.dark ? "#f18b91" : "#b42332"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            Controls.TextField { id: repositoryUrl; objectName: "repositoryUrl"; placeholderText: "https://…/repository.flatpakrepo"; Accessible.name: "Repository URL"; Layout.fillWidth: true }
            Controls.Label {
                objectName: "repositoryUrlError"
                text: "Enter an HTTPS .flatpakrepo URL."
                visible: repositoryUrl.text.length > 0 && !addRepositoryDialog.validUrl
                color: root.dark ? "#f18b91" : "#b42332"
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            ThemedComboBox { id: repositoryScope; objectName: "repositoryScope"; model: ["User", "System"]; Accessible.name: "Installation scope"; Layout.fillWidth: true }
        }
    }
    ThemedDialog {
        id: sourceFailuresDialog
        objectName: "sourceFailuresDialog"
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 620)
        height: Math.min(root.height - 32, 440, Math.max(190, 100 + Math.min(root.readFailures.length, 4) * 80 + (root.expandedFailure ? 45 : 0)))
        modal: true
        title: "Source checks"
        standardButtons: Controls.Dialog.Close
        onOpened: root.expandedFailure = ""
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 10
            Controls.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                contentWidth: availableWidth
                clip: true
                Column {
                    width: parent.width
                    spacing: 10
                    Repeater {
                        model: root.readFailures
                        delegate: Column {
                            required property var modelData
                            width: parent.width
                            spacing: 4
                            RowLayout {
                                width: parent.width
                                Controls.Label {
                                    text: root.sourceDisplayName(modelData.source)
                                    color: root.ink
                                    font.bold: true
                                    Layout.fillWidth: true
                                }
                                ActionButton {
                                    text: "Retry"
                                    symbol: "refresh"
                                    enabled: !backend.busy && modelData.kind !== "unsupported" && !(root.currentView === "Search" && root.queryDirty)
                                    onClicked: root.retryFailedSource(modelData.source)
                                }
                                ActionButton {
                                    text: root.expandedFailure === modelData.source ? "Hide help" : "Help"
                                    symbol: "help"
                                    onClicked: root.expandedFailure = root.expandedFailure === modelData.source ? "" : modelData.source
                                }
                            }
                            Controls.Label {
                                width: parent.width
                                text: modelData.kind === "unsupported" ? "This source does not support this view." :
                                    modelData.kind === "authorization" ? "Authorization is required." :
                                    modelData.kind === "locked" ? "The package manager is busy." :
                                    modelData.kind === "cancelled" ? "The check was cancelled." : "The source could not be checked."
                                color: root.muted
                            }
                            Controls.Label {
                                width: parent.width
                                text: root.failureHelp(modelData.kind)
                                textFormat: Text.PlainText
                                wrapMode: Text.WordWrap
                                color: root.muted
                                visible: root.expandedFailure === modelData.source
                            }
                        }
                    }
                }
            }
            ActionButton {
                text: "Copy diagnostics"
                symbol: "help"
                onClicked: { diagnosticsText.selectAll(); diagnosticsText.copy(); diagnosticsText.deselect(); }
            }
            TextEdit {
                id: diagnosticsText
                visible: false
                readOnly: true
                text: root.copyableDiagnostics()
            }
        }
    }
    ThemedDialog {
        id: screenshotDialog
        objectName: "screenshotDialog"
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 1040)
        height: Math.min(root.height - 32, 720)
        modal: true
        title: root.screenshotCaption || "Screenshot"
        standardButtons: Controls.Dialog.Close
        onClosed: { root.screenshotUrl = ""; root.restoreDialogFocus(); }
        background: Rectangle { color: root.surface; radius: 10; border.color: root.line }
        contentItem: Item {
            Image {
                id: fullScreenshot
                anchors.fill: parent
                source: screenshotDialog.visible ? root.screenshotUrl : ""
                asynchronous: true
                cache: true
                sourceSize.width: 960
                sourceSize.height: 540
                fillMode: Image.PreserveAspectFit
            }
            Controls.BusyIndicator {
                anchors.centerIn: parent
                running: fullScreenshot.status === Image.Loading && root.motionEnabled
                visible: running
            }
            Controls.Label {
                anchors.centerIn: parent
                color: root.muted
                text: fullScreenshot.status === Image.Error ? "Screenshot unavailable" : "Loading…"
                visible: fullScreenshot.status === Image.Error || (fullScreenshot.status === Image.Loading && !root.motionEnabled)
            }
        }
    }
    ThemedDialog {
        id: confirmation
        objectName: "confirmationDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 600)
        height: Math.min(root.height - 32, Math.max(230, confirmationBody.implicitHeight + 130))
        background: Rectangle { color: root.surface; radius: 12; border.color: root.line }
        title: "Confirm changes"
        modal: true
        enter: Transition {
            ParallelAnimation {
                NumberAnimation { property: "opacity"; from: 0; to: 1; duration: root.revealDuration; easing.type: Easing.OutCubic }
                NumberAnimation { property: "scale"; from: root.motionEnabled ? 0.97 : 1; to: 1; duration: root.revealDuration; easing.type: Easing.OutCubic }
            }
        }
        exit: Transition {
            NumberAnimation { property: "opacity"; to: 0; duration: root.feedbackDuration; easing.type: Easing.OutCubic }
        }
        readonly property var preview: JSON.parse(backend.confirmation_data || "{}")
        readonly property string summaryText: preview.summary || preview.body || backend.confirmation
        readonly property var summaryLines: summaryText.split("\n")
        property bool detailsExpanded: false
        standardButtons: Controls.Dialog.NoButton
        onClosed: root.restoreDialogFocus()
        onOpened: {
            detailsExpanded = false;
            confirmationCancel.forceActiveFocus();
        }
        onAccepted: backend.confirm(true)
        onRejected: backend.confirm(false)
        Shortcut {
            sequence: "Alt+" + (confirmation.preview.action || "Apply").trim().charAt(0).toUpperCase()
            enabled: confirmation.visible
            onActivated: confirmation.accept()
        }
        Shortcut {
            sequence: "Alt+C"
            enabled: confirmation.visible
            onActivated: confirmation.reject()
        }
        footer: Item {
            implicitHeight: 54
            RowLayout {
                anchors.fill: parent
                anchors.margins: 8
                spacing: 8
                Item { Layout.fillWidth: true }
                ActionButton {
                    objectName: "confirmationApply"
                    text: (confirmation.preview.action || "Apply").trim().split(/\s+/)[0]
                    symbol: ""
                    primary: true
                    Layout.minimumWidth: 80
                    onClicked: confirmation.accept()
                }
                ActionButton {
                    id: confirmationCancel
                    objectName: "confirmationCancel"
                    text: "Cancel"
                    symbol: ""
                    Layout.minimumWidth: 80
                    onClicked: confirmation.reject()
                }
            }
        }
        contentItem: Controls.ScrollView {
            id: confirmationScroll
            contentWidth: availableWidth
            contentHeight: confirmationBody.implicitHeight + 32
            clip: true
            ColumnLayout {
                id: confirmationBody
                x: 20
                y: 16
                width: Math.max(0, confirmationScroll.availableWidth - 40)
                spacing: 12
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: summaryContent.implicitHeight + 28
                    color: root.selection
                    radius: 8
                    ColumnLayout {
                        id: summaryContent
                        anchors.fill: parent
                        anchors.margins: 14
                        spacing: 6
                        Controls.Label {
                            objectName: "confirmationSummary"
                            Layout.fillWidth: true
                            color: root.ink
                            font.bold: true
                            text: confirmation.summaryLines[0] || ""
                            wrapMode: Text.WrapAnywhere
                            textFormat: Text.PlainText
                        }
                        Controls.Label {
                            objectName: "confirmationSummaryMeta"
                            visible: text.length > 0
                            Layout.fillWidth: true
                            color: root.muted
                            text: confirmation.summaryLines.slice(1).join("\n").trim()
                            wrapMode: Text.WrapAnywhere
                            textFormat: Text.PlainText
                        }
                    }
                }
                ActionButton {
                    objectName: "confirmationDetailsButton"
                    visible: !!confirmation.preview.details
                    text: confirmation.detailsExpanded ? "Hide details" : "Show details"
                    symbol: ""
                    onClicked: confirmation.detailsExpanded = !confirmation.detailsExpanded
                }
                Rectangle {
                    visible: confirmation.detailsExpanded && !!confirmation.preview.details
                    Layout.fillWidth: true
                    implicitHeight: confirmationDetails.implicitHeight + 28
                    color: root.surface
                    radius: 8
                    border.color: root.line
                    Controls.Label {
                        id: confirmationDetails
                        objectName: "confirmationDetails"
                        anchors.fill: parent
                        anchors.margins: 14
                        color: root.muted
                        text: confirmation.preview.details || ""
                        wrapMode: Text.WrapAnywhere
                        textFormat: Text.PlainText
                    }
                }
            }
        }
    }
    Shortcut {
        sequence: "Ctrl+Q"
        onActivated: root.close()
    }
    Shortcut {
        sequence: "Ctrl+L"
        enabled: resultsBox.visible && results.count > 0 && !root.retainingResults
        onActivated: results.forceActiveFocus()
    }
    Shortcut {
        sequence: "Ctrl+F"
        onActivated: {
            if (root.currentView === "Installed") {
                installedFilterField.forceActiveFocus();
                installedFilterField.selectAll();
            } else if (["Updates", "Clean"].indexOf(root.currentView) >= 0) {
                root.rememberDialogFocus();
                sourcePopup.open();
                Qt.callLater(() => pickerSearch.forceActiveFocus());
            } else {
                if (root.currentView !== "Search")
                    root.openView("Search");
                searchPane.focusSearch(true);
            }
        }
    }
    Shortcut {
        sequence: "Ctrl+1"
        onActivated: root.openView("Search")
    }
    Shortcut {
        sequence: "Ctrl+2"
        onActivated: root.openView("Installed")
    }
    Shortcut {
        sequence: "Ctrl+3"
        onActivated: root.openView("Updates")
    }
    Shortcut {
        sequence: "Ctrl+4"
        onActivated: root.openView("Clean")
    }
    Shortcut {
        sequence: "Ctrl+5"
        onActivated: root.openView("Sources")
    }
    Shortcut {
        sequence: "Ctrl+R"
        onActivated: root.reload(true)
    }
    Shortcut {
        sequence: "Ctrl+I"
        enabled: !backend.busy || backend.writing
        onActivated: root.propose("install")
    }
    Shortcut {
        sequence: "Ctrl+D"
        enabled: !backend.busy || backend.writing
        onActivated: root.propose("remove")
    }
    Shortcut {
        sequence: "Ctrl+U"
        enabled: !backend.busy || backend.writing
        onActivated: root.propose("upgrade")
    }
    Shortcut {
        sequence: "Ctrl+Shift+U"
        enabled: root.currentView === "Updates" && (!backend.busy || backend.writing) && root.selectedCount() > 0 && (root.uncheckedPackages.length > 0 || backend.upgradable)
        onActivated: root.upgradeUpdates()
    }
    Shortcut {
        sequence: "Ctrl+M"
        enabled: !backend.busy || backend.writing
        onActivated: root.propose("refresh")
    }
    Shortcut {
        sequence: "Escape"
        enabled: backend.busy
        onActivated: backend.cancel()
    }
}
