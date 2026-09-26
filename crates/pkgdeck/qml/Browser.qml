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
        nameFilters: ["Packages and sources (" + root.supportedFilePatterns().join(" ") + ")"]
        onAccepted: root.openExternalInput(selectedFile.toString())
    }
    ThemedDialog {
        id: addPackageDialog
        objectName: "addPackageDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 480)
        title: "Install from a file or link"
        modal: true
        standardButtons: Controls.Dialog.Cancel
        onAboutToShow: packageLink.text = ""
        onOpened: packageLink.forceActiveFocus()
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 12
            ThemedTextField {
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
    // Stored preferences, for pages in their own files.
    readonly property var store: preferences
    SystemPalette { id: disabledPalette; colorGroup: SystemPalette.Disabled }
    required property var backend
    readonly property var actionProgress: JSON.parse(backend.progress || "{}")
    readonly property var repositoryReport: JSON.parse(backend.repositories || "{}")
    readonly property var repositoryFeatures: repositoryReport.features || ({})
    property bool desktopAutostartSupported: Qt.platform.os !== "osx"
    property bool systemAuthorizationSupported: Qt.platform.os !== "osx"
    function managerAvailable(id) { return sourceInfo(id).availability_kind === "available"; }
    function supportedFilePatterns() {
        // AppImage is always available on Linux, including before source discovery.
        if (sourceCatalog.length === 0)
            return Qt.platform.os === "linux" ? ["*.AppImage"] : [];
        let patterns = [];
        if (managerAvailable("appimage")) patterns.push("*.AppImage");
        if (managerAvailable("apt")) patterns.push("*.deb", "*.sources", "*.list");
        if (managerAvailable("dnf") || managerAvailable("zypper")) patterns.push("*.rpm", "*.repo");
        if (managerAvailable("pacman")) patterns.push("*.pkg.tar.zst", "*.pkg.tar.xz", "*.pkg.tar.gz", "*.pkg.tar.bz2", "*.pkg.tar.lz4");
        if (managerAvailable("flatpak")) patterns.push("*.flatpak", "*.flatpakref", "*.flatpakrepo");
        if (managerAvailable("snap")) patterns.push("*.snap");
        if (managerAvailable("zypper")) patterns.push("*.ymp");
        return patterns;
    }
    readonly property bool repositorySourcesAvailable: ["flatpak", "fwupd", "apt", "dnf", "zypper"].some((id) => managerAvailable(id))
    function repositoryEditable(row) {
        if (row.backend === "fwupd") return true;
        if (row.backend !== "flatpak") return false;
        return row.scope === "system" ? !!repositoryFeatures.flatpak_system : !!repositoryFeatures.flatpak_user;
    }
    function repositoryChange(row, action, extra) {
        const request = Object.assign({backend: row.backend, name: row.name, scope: row.scope, action: action}, extra || {});
        backend.changeRepository(JSON.stringify(request));
    }
    property string currentView: "Search"
    readonly property var activityRows: JSON.parse(backend.activity || "[]")
    readonly property int queuedCount: activityRows.filter(row => row.state === "queued").length
    readonly property var backgroundState: JSON.parse(backend.background_state && backend.background_state !== "{}" ? backend.background_state : preferences.lastBackgroundState)
    property bool notificationAvailable: false
    signal testNotificationRequested()
    property bool startHidden: Qt.application.arguments.indexOf("--background") >= 0
    property bool forceQuit: false
    property bool trayAvailable: false
    property alias backgroundMode: preferences.backgroundMode
    property alias autostartEnabled: preferences.autostart
    property alias preferredSidebarWidth: preferences.sidebarWidth
    function showFromTray() {
        root.show();
        root.raise();
        root.requestActivate();
    }
    function toggleFromTray() {
        if (root.visible)
            root.hide();
        else
            root.showFromTray();
    }
    function checkUpdates(force) {
        const offline = NetworkInformation.reachability === NetworkInformation.Reachability.Disconnected || NetworkInformation.isBehindCaptivePortal;
        backend.checkUpdates(root.checkedSources().join(","), preferences.backgroundMode, offline, NetworkInformation.isMetered, force === true);
    }
    property string resultView: "Search"
    // Large sections come back as the same text when revisited; reuse
    // their parsed rows instead of parsing megabytes again on every switch.
    // A plain holder, so caching never notifies bindings.
    readonly property var parsedRowsCache: ({entries: []})
    function parseRows(text) {
        if (!text || text.length < 65536)
            return JSON.parse(text || "[]");
        const entries = parsedRowsCache.entries;
        for (const entry of entries) {
            if (entry.text === text)
                return entry.rows;
        }
        const rows = JSON.parse(text);
        parsedRowsCache.entries = [{text: text, rows: rows}].concat(entries).slice(0, 4);
        return rows;
    }
    property var liveItems: parseRows(backend.rows)
    property var retainedItems: []
    property bool retainingResults: false
    property string resultQuery: ""
    // The query the retained rows were loaded for.
    property string retainedQuery: ""
    // A refresh of the same rows keeps them until the answer covers as many
    // rows or is complete, so the first source to reply never collapses the
    // list to its own rows.
    property bool retainUntilDone: false
    property var items: currentView === resultView ? (retainingResults ? retainedItems : liveItems) : []
    property bool reduceMotion: preferences.reduceMotion
    readonly property bool motionEnabled: Theme.motionEnabled
    readonly property int feedbackDuration: Theme.feedbackDuration
    readonly property int revealDuration: Theme.revealDuration
    property var beforeWrite: null
    property var completedRows: []
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
            pageSwitch.complete();
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
        // Later: the catalog can change while the window is still being built.
        Qt.callLater(root.rememberSearchHint);
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
        // A stopped load is not a failure: see loadStopped.
        return failures.filter((failure) => failure.kind !== "cancelled" && effectiveSources().indexOf(failure.source) >= 0);
    }
    // The last load was cancelled (the Cancel button, or a new page).
    readonly property bool loadStopped: currentView === resultView && !backend.busy
        && (reportState.failures || []).some((failure) => failure.kind === "cancelled")
    function sourceFailureTitle() {
        return readFailures.length === 1
            ? "Couldn't check " + sourceDisplayName(readFailures[0].source)
            : "Couldn't check " + readFailures.length + " sources";
    }
    function failureSummary(id) {
        const row = items.find((item) => item.kind === "failure" && item.source === id);
        const reportFailure = (reportState.failures || []).find((failure) => failure.source === id);
        return (row && row.summary) || (reportFailure && reportFailure.detail) || (currentView === "Sources" ? sourceInfo(id).summary : "") || "This source could not be checked.";
    }
    function lastSuccessfulCheck(id) {
        const seconds = (reportState.last_success || {})[id];
        return seconds ? new Date(seconds * 1000).toLocaleString() : "No successful check yet";
    }
    function copyableDiagnostics() {
        return "View: " + currentView + "\nState: " + (reportState.phase || "unknown") + "\n" +
            readFailures.map((failure) => failure.source + " (" + failure.kind + "): " + failureSummary(failure.source)).join("\n");
    }
    function retryFailedSource(id) {
        if (currentView === "Search" && queryDirty)
            return;
        const query = currentView === "Search" ? searchPane.text : "";
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
        // Sources that answered still give their good news; the ones that
        // failed show as a warning under it (see sourceFailureEmptyHint).
        if (readFailures.length > 0 && (!someSourcesChecked() || reportState.phase === "failed" || ["Updates", "Clean"].indexOf(currentView) < 0))
            return sourceFailureTitle();
        if (readFailures.length > 0 && items.filter((row) => row.kind !== "failure").length === 0) {
            const names = checkedSourceNames();
            const who = names.length <= 2 ? names.join(" and ") : names.length + " other sources";
            return currentView === "Updates"
                ? who + (names.length === 1 ? " is" : " are") + " up to date"
                : "Nothing to clean in " + who;
        }
        if (loadStopped && items.length === 0)
            return currentView === "Search" ? "Search stopped" : "Loading stopped";
        if (reportState.phase === "unsupported")
            return "None of your enabled sources support this page.";
        if (currentView === "Search" && queryDirty)
            return "";
        if (currentView === "Search" && searchPane.text.trim().length === 0)
            return "";
        if (currentView === "Installed" && (installedFilter.length > 0 || multiSourceOnly))
            return "No packages match these filters.";
        if (viewSourceFilters[currentView] && items.length > 0)
            return "No results from selected sources.";
        if (currentView === "Updates")
            return "You're up to date";
        if (currentView === "Clean")
            return "Nothing to clean";
        if (currentView === "Installed")
            return "No installed packages.";
        if (currentView === "Sources")
            return "No available sources.";
        return currentView === "Search" ? "No matching packages." : "No results to show.";
    }
    // At least one source this page asks answered the last load.
    function checkedSourceNames() {
        const failed = readFailures.map((failure) => failure.source);
        return effectiveSources().filter((id) => failed.indexOf(id) < 0
            && sourceInfo(id).availability_kind === "available" && sourceSupportsView(id)).map((id) => sourceDisplayName(id));
    }
    function someSourcesChecked() {
        return checkedSourceNames().length > 0;
    }
    // "Checked 3 minutes ago", from when the shown rows were loaded.
    function checkedAgo() {
        const at = reportState.checked_at;
        if (!at)
            return "";
        const seconds = Math.max(0, Math.round(clock.now / 1000 - at));
        if (seconds < 60)
            return "Checked just now";
        const minutes = Math.round(seconds / 60);
        if (minutes < 60)
            return "Checked " + minutes + (minutes === 1 ? " minute ago" : " minutes ago");
        const hours = Math.round(minutes / 60);
        if (hours < 24)
            return "Checked " + hours + (hours === 1 ? " hour ago" : " hours ago");
        return "Checked " + new Date(at * 1000).toLocaleString(Qt.locale(), Locale.ShortFormat);
    }
    QtObject {
        id: clock
        property real now: Date.now()
    }
    Timer {
        interval: 30000
        repeat: true
        running: root.visible
        onTriggered: clock.now = Date.now()
    }
    function resultsHeading() {
        if (backend.writing)
            return "Applying changes…";
        if (backend.busy)
            return currentView === "Search" ? "Searching…" :
                (retainingResults || viewItems.length > 0 ? "Refreshing…" : "Loading…");
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
    // A failing source can be turned off while another one stays on.
    function canTurnOff(id) {
        return checkedSources().indexOf(id) >= 0 && checkedSources().length > 1;
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
    // The empty Search page's second line: how many sources a search covers.
    function rememberSearchHint() {
        if (sourceCatalog.length > 0)
            preferences.searchHint = root.searchHint();
    }
    // Until sources are checked, repeat the last known hint so the prompt
    // appears with the window instead of after the check.
    function searchHint() {
        if (sourceCatalog.length === 0)
            return preferences.searchHint || "Searches your sources as you type.";
        const names = effectiveSources("Search")
            .filter((id) => sourceInfo(id).availability_kind === "available" && sourceInfo(id).capabilities.indexOf("search") >= 0)
            .map((id) => sourceDisplayName(id));
        if (names.length === 0)
            return "No enabled source can search. Turn one on in Sources.";
        return "Searches " + (names.length === 1 ? names[0] : names.length + " sources") + " as you type.";
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
    // Picker rows with their section, and whether each starts one.
    function pickerSections() {
        let previous = "";
        return pickerItems().map((id) => {
            const usable = sourceInfo(id).availability_kind === "available" && sourceSupportsView(id);
            const section = usable ? sourceCategory(id) : "Unavailable";
            const row = {id: id, section: section, showHeader: section !== previous};
            previous = section;
            return row;
        });
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
    // Only the Updates checkboxes use identities; other pages skip the work.
    readonly property var allPackageIdentities: {
        const all = [];
        const seen = new Set();
        const rows = root.currentView === "Updates" ? root.items : [];
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
        else {
            markActiveRows(checkedIdentities());
            backend.proposeChecked(JSON.stringify(checkedIdentities().map((id) => JSON.parse(id))));
        }
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
            const named = group.find((member) => !!member.display_name);
            const title = named ? named.display_name
                : group.reduce((best, member) => member.name.length < best.length ? member.name : best, group[0].name);
            const sources = [...new Set(group.map((member) => root.sourceDisplayName(member.source)))];
            for (let i = 0; i < group.length; i++)
                grouped.push(Object.assign({}, group[i], {groupStart: i === 0, groupTitle: title, groupCount: group.length, groupSources: sources}));
        }
        return grouped;
    }
    // Lower-cased filter text per parsed row, computed once per load.
    readonly property var searchKeys: new WeakMap()
    function searchKey(row) {
        let key = searchKeys.get(row);
        if (key === undefined) {
            key = ((row.name || "") + " " + (row.display_name || "") + " " + (row.summary || "") + " " + (row.source || "")).toLowerCase();
            searchKeys.set(row, key);
        }
        return key;
    }
    readonly property var cleanupFailures: currentView === "Clean" ? items.filter(row => row.kind === "failure") : []
    property var viewItems: {
        let rows = root.currentView === "Search" ? items.filter((row) => row.kind !== "failure" && !isFabricated(row)) : items.filter((row) => row.kind !== "failure");
        if (root.currentView === "Clean")
            rows = rows.filter(row => row.kind === "cleanup");
        if (root.currentView === "Sources" && !root.showUnavailableSources)
            rows = rows.filter(row => row.kind !== "source" || row.available);
        if (root.currentView !== "Sources") {
            const shown = new Set(root.effectiveSources());
            rows = rows.filter((row) => shown.has(row.source));
        }
        // The Installed filter narrows the loaded rows as you type; the
        // backend is queried once with an empty query (see reload).
        if (root.currentView === "Installed") {
            const filter = root.installedFilter.trim().toLowerCase();
            if (filter !== "") {
                const matches = (row) => root.searchKey(row).indexOf(filter) >= 0;
                const matchingGroups = new Set(rows.filter((row) => row.kind === "package" && matches(row)).map((row) => row.same_app_group).filter(Boolean));
                rows = rows.filter((row) => row.kind !== "package" || matchingGroups.has(row.same_app_group) || matches(row));
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
            // Previous matches narrow instantly while the new query runs.
            const shownFor = (root.retainingResults ? root.retainedQuery : root.resultQuery).toLowerCase();
            if (query !== "" && shownFor !== query)
                rows = rows.filter((row) => ((row.name || "") + " " + (row.display_name || "") + " " + (row.summary || "")).toLowerCase().indexOf(query) >= 0);
            if (query !== "")
                rows.sort((a, b) => ((isFabricated(a) ? 1 : 0) - (isFabricated(b) ? 1 : 0)) || (relevanceScore(a, query) - relevanceScore(b, query)) || relevanceTiebreak(a, b));
        }
        // One app offered by several sources reads as one group, app first.
        const appFirst = root.currentView === "Search" && sortColumn === "" && searchPane.text.trim() !== "";
        return root.currentView === "Installed" || appFirst ? groupInstalledRows(rows) : rows;
    }
    // The list shows viewItems through resultsModel, updated in place by
    // row identity: rows that stay keep their delegates (and scroll
    // position), and inserts, removals and moves can animate.
    ListModel { id: resultsModel }
    property var modelKeys: []
    property var modelJson: []
    // Few changes animate; bulk loads replace the rows at once.
    property bool animateListChanges: false
    function rowKeys(rows) {
        const seen = new Map();
        return rows.map((row) => {
            const base = row.kind + "|" + rowIdentity(row);
            const count = seen.get(base) || 0;
            seen.set(base, count + 1);
            return count ? base + "#" + count : base;
        });
    }
    function syncResults() {
        const rows = viewItems;
        const keys = rowKeys(rows);
        const json = rows.map((row) => JSON.stringify(row));
        const wanted = new Set(keys);
        const kept = modelKeys.filter((key) => wanted.has(key)).length;
        // Mostly new rows (another page, a new search): replace them all.
        if (modelKeys.length === 0 || kept < Math.min(modelKeys.length, keys.length) / 2) {
            animateListChanges = false;
            resultsModel.clear();
            resultsModel.append(json.map((text) => ({rowJson: text})));
            modelKeys = keys;
            modelJson = json;
            results.forceLayout();
            return;
        }
        const currentKeys = modelKeys.slice();
        const currentJson = modelJson.slice();
        animateListChanges = Math.abs(currentKeys.length - keys.length) + (currentKeys.length - kept) < 12;
        for (let i = currentKeys.length - 1; i >= 0; i--) {
            if (!wanted.has(currentKeys[i])) {
                resultsModel.remove(i);
                currentKeys.splice(i, 1);
                currentJson.splice(i, 1);
            }
        }
        let moves = 0;
        for (let i = 0; i < keys.length; i++) {
            if (currentKeys[i] !== keys[i]) {
                const from = currentKeys.indexOf(keys[i], i + 1);
                if (from >= 0) {
                    // A re-sort moves many rows: replace them all instead.
                    if (++moves > 48) {
                        animateListChanges = false;
                        resultsModel.clear();
                        resultsModel.append(json.map((text) => ({rowJson: text})));
                        modelKeys = keys;
                        modelJson = json;
                        results.forceLayout();
                        return;
                    }
                    resultsModel.move(from, i, 1);
                    currentKeys.splice(i, 0, currentKeys.splice(from, 1)[0]);
                    currentJson.splice(i, 0, currentJson.splice(from, 1)[0]);
                } else {
                    resultsModel.insert(i, {rowJson: json[i]});
                    currentKeys.splice(i, 0, keys[i]);
                    currentJson.splice(i, 0, json[i]);
                    continue;
                }
            }
            if (currentJson[i] !== json[i]) {
                resultsModel.setProperty(i, "rowJson", json[i]);
                currentJson[i] = json[i];
            }
        }
        modelKeys = keys;
        modelJson = json;
        results.forceLayout();
    }
    // Rows the change being confirmed or run applies to, by identity.
    property var activeRows: []
    function markActiveRows(identities) {
        activeRows = identities;
    }
    Connections {
        target: backend
        function onWritingChanged() {
            if (!backend.writing)
                root.activeRows = [];
        }
    }
    // Rows the running change names, from its progress.
    readonly property var progressTargets: (actionProgress.targets || []).map((row) => rowIdentity(row))
    function rowIsActive(identity) {
        return backend.writing && (activeRows.indexOf(identity) >= 0 || progressTargets.indexOf(identity) >= 0);
    }
    // Share of the running change that is done, or -1 when unknown.
    function actionFraction() {
        const p = actionProgress;
        if (typeof p.fraction === "number")
            return Math.max(0, Math.min(1, p.fraction));
        if ((p.transfer_total || 0) > 0)
            return Math.min(1, (p.transferred || 0) / p.transfer_total);
        if ((p.total || 0) > 1)
            return Math.min(1, (p.done || 0) / p.total);
        return -1;
    }
    // Rows show a focus ring only while the keyboard moves through them.
    property bool keyboardNavigation: false
    readonly property int groupHeaderHeight: 38
    readonly property int iconSlotSize: compact ? 40 : 32
    function rowHeight(row) {
        return (compact ? (row.kind === "source" ? Math.max(68, font.pointSize * 5.5) : Math.max(94, font.pointSize * 8.5))
            : Math.max(56, font.pointSize * 5)) + (row.groupStart ? groupHeaderHeight : 0);
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
    // The sidebar is resizable. Narrow windows, or a sidebar dragged
    // narrow, show it as an icon rail instead.
    readonly property int railWidth: 64
    readonly property int defaultSidebarWidth: 212
    // Dragging or keying the sidebar into the rail slides; a window
    // resize switches at once so the page never lags behind the window.
    onSidebarRailChanged: if (width >= 820) railSwitch.restart()
    Timer { id: railSwitch; interval: Theme.revealDuration + 40 }
    // Wide enough for the title and the footer at any font size.
    readonly property int sidebarMinimumWidth: Math.ceil(Math.max(180,
        28 + 10 + sidebarTitle.implicitWidth + 4 + 28,
        sidebarSignature.implicitWidth + 4 + 28))
    // A sidebar dragged narrower than this becomes the icon rail.
    readonly property int railThreshold: 150
    // Widening the sidebar stops where the page would switch to its compact
    // layout, so dragging never shifts the page. The limit shrinks with the
    // window down to the minimum width and never jumps back up.
    readonly property int sidebarMaximumWidth: Math.max(sidebarMinimumWidth, Math.min(360, width - mediumWidth))
    readonly property bool sidebarRail: width < 820 || preferences.sidebarWidth < railThreshold
    readonly property int sidebarWidth: sidebarRail ? railWidth
        : Math.max(sidebarMinimumWidth, Math.min(sidebarMaximumWidth, preferences.sidebarWidth))
    // Layout follows the space the page has, not the window.
    readonly property real pageWidth: width - sidebarWidth
    // Three layout steps: a full table, a table without the summary
    // column below mediumWidth, and stacked cards below compactWidth.
    readonly property int mediumWidth: 748
    readonly property int compactWidth: 560
    readonly property bool medium: !compact && pageWidth < mediumWidth
    // Fixed while the full sidebar shows, so resizing the window does not
    // shift the page; only windows narrow enough for the icon rail use less.
    readonly property int pageMargin: width < 820 ? 12 : 28
    readonly property bool compact: pageWidth < compactWidth
    // Header actions keep only their icons when the page is this narrow.
    readonly property bool headerIconsOnly: pageWidth < 600
    readonly property int shortListLimit: compact ? 3 : 8
    // Height of the list card's heading and column headers.
    function listChromeHeight() {
        return resultsHeadingRow.implicitHeight + 28 + 1 + (compact ? 0 : columnHeaderRow.implicitHeight + 20) + 8;
    }
    function shortResultsHeight() {
        return Math.max(130, listChromeHeight() + viewItems.reduce((height, row) => height + rowHeight(row), 0));
    }
    // With details open the list keeps this many rows; details scroll instead.
    // Height left for the results and details together: the page minus
    // every other visible part (header, filters, banners, actions) and the
    // spacing between them. Both minimums below come out of this budget, so
    // opening details never pushes anything off a short window.
    function detailsBudget() {
        let used = 0;
        let shown = 0;
        for (const child of pageContent.children) {
            // The filler takes no height of its own, only a gap (below).
            // Children that fill the page take what is left, so they are not "used".
            if (!child.visible || child === resultsBox || child === detailsPanel || child.objectName === "pageFiller"
                    || child === searchEmptyState || child === settingsScroll)
                continue;
            used += child.height;
            shown++;
        }
        // The window's space, not pageContent.height: the page grows past the
        // window to fit its children's minimums, which would feed back here.
        const page = root.contentItem.height - 2 * pageContent.Layout.margins;
        const filler = pageContent.children.some((child) => child.objectName === "pageFiller" && child.visible);
        return Math.max(0, page - used - pageContent.spacing * (shown + 1 + (filler ? 1 : 0)));
    }
    function detailsMinimumHeight() {
        return Math.min(120, detailsPanel.idealHeight, detailsBudget() * 0.4);
    }
    function detailsListHeight() {
        const rows = compact ? 2 : 3;
        const rowHeight = compact ? Math.max(94, font.pointSize * 8.5) : Math.max(56, font.pointSize * 5);
        const budget = detailsBudget();
        // Compact pages leave most of the space to the details' gallery.
        return Math.min(shortResultsHeight(), listChromeHeight() + rows * rowHeight,
            budget * (compact ? 0.3 : 0.55), budget - detailsMinimumHeight());
    }
    // Tokens live in the Theme singleton; these aliases keep bindings short.
    Binding { target: Theme; property: "appearance"; value: preferences.appearance }
    Binding { target: Theme; property: "reduceMotion"; value: root.reduceMotion }
    Binding { target: Theme; property: "baseFont"; value: root.font }
    readonly property bool systemAppearance: Theme.systemAppearance
    readonly property bool dark: Theme.dark
    readonly property color canvas: Theme.canvas
    readonly property color surface: Theme.surface
    readonly property color ink: Theme.ink
    readonly property color muted: Theme.muted
    readonly property color accent: Theme.accent
    function tint(base, alpha) { return Theme.tint(base, alpha); }
    readonly property color line: Theme.line
    readonly property color strongLine: Theme.strongLine
    readonly property color hoverTint: Theme.hoverTint
    readonly property color selection: Theme.selection
    readonly property color accentInk: Theme.accentInk
    readonly property color danger: Theme.danger
    readonly property color success: Theme.success
    readonly property color warning: Theme.warning
    readonly property int controlRadius: Theme.controlRadius
    readonly property int cardRadius: Theme.cardRadius
    readonly property int controlHeight: Theme.controlHeight
    color: canvas
    palette.window: canvas
    palette.base: surface
    palette.text: ink
    palette.windowText: systemAppearance ? systemPalette.windowText : ink
    palette.buttonText: systemAppearance ? systemPalette.buttonText : ink
    palette.button: systemAppearance ? systemPalette.button : surface
    palette.highlight: accent
    palette.highlightedText: accentInk

    // Display names for backend ids used by related-install indicators without
    // changing the rows' exact identities.
    function sourceDisplayName(id) {
        const at = knownSourceIds.indexOf(id);
        return at >= 0 ? sourceNames[at] : id;
    }
    // Flatpak branches other than "stable" (SDK extensions, runtimes) are
    // shown, or several rows with one name and version look identical.
    function flatpakBranch(row) {
        const parts = row.source === "flatpak" && row.reference ? row.reference.split("/") : [];
        const branch = parts.length >= 3 ? parts[parts.length - 1] : "";
        return branch && branch !== "stable" ? branch : "";
    }
    function sourceLine(row) {
        const scoped = row.source === "flatpak" || containerSource(row.source);
        return [sourceDisplayName(row.source), row.remote || "", scoped ? (row.scope === "system" ? "System" : "User") : "", flatpakBranch(row)]
            .filter(Boolean).join(", ");
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
                return row.installed || row.candidate || "Unknown";
            return row.installed + " → " + row.candidate;
        }
        if (isInstalled(row))
            return row.installed || "Unknown";
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
    title: "PkgDeck"
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
    // Activity slides in over the current page instead of replacing it.
    property bool activityOpen: false
    function openView(view) {
        if (view === "Activity") {
            activityOpen = true;
            backend.refreshActivity();
            return;
        }
        activityOpen = false;
        const changed = currentView !== view;
        queryDirty = false;
        selectedIdentity = null;
        uncheckedPackages = [];
        if (changed) {
            retainingResults = false;
            retainedItems = [];
            completedRows = [];
        }
        currentView = view;
        results.currentIndex = -1;
        if (view === "Search") {
            if (changed)
                reload();
            searchPane.focusSearch(false);
        } else if (["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(view) >= 0)
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
        // While typing, keep the previous matches on screen (narrowed below)
        // until the new ones arrive, instead of flashing an empty list.
        const refining = currentView === "Search" && !emptySearch;
        retainedItems = currentView === resultView && (sameQuery || refining) ? items.slice() : [];
        retainingResults = retainedItems.length > 0;
        retainUntilDone = retainingResults && sameQuery;
        retainedQuery = resultQuery;
        // A new page shows its rows once they are loaded (below), so the
        // previous page's rows are never filtered and laid out as its own.
        const nextResultView = clearSearchResults ? "" : currentView;
        if (resultView !== currentView)
            resultView = nextResultView === currentView ? "" : nextResultView;
        if (currentView === "Search")
            resultQuery = searchPane.text.trim();
        results.currentIndex = -1;
        if (!preserveSelection)
            selectedIdentity = null;
        uncheckedPackages = [];
        // Installed filtering is client-side over the loaded rows (see
        // viewItems), so the backend always returns the full installed set
        // and typing never triggers a native query.
        backend.load(currentView, currentView === "Search" ? searchPane.text : "", currentView === "Sources" ? "" : checkedCsv(), useSudo, force === true);
        resultView = nextResultView;
        if (!backend.busy)
            retainingResults = false;
    }
    readonly property var listViews: ["Search", "Installed", "Updates", "Clean", "Sources"]
    // The action a row's button runs: "install", "remove", "upgrade",
    // "clean", or "" when the row has none.
    function rowActionName(row) {
        if (!row)
            return "";
        if (row.kind === "cleanup")
            return "clean";
        if (row.kind !== "package")
            return "";
        if (updateOnly(row.source))
            return row.update === "available" ? "upgrade" : "";
        if (currentView === "Updates")
            return "upgrade";
        return isInstalled(row) ? "remove" : "install";
    }
    function runRowAction(index) {
        const row = viewItems[index];
        const action = rowActionName(row);
        if (!action || retainingResults || (backend.busy && !backend.writing))
            return;
        markActiveRows([rowIdentity(row)]);
        backend.propose(action, originalIndex(index));
    }
    function choose(index) {
        if (retainingResults || index < 0 || index >= viewItems.length)
            return;
        const identity = rowIdentity(viewItems[index]);
        // Arrowing into the row that is already open must not load it again.
        const same = results.currentIndex === index && selectedIdentity === identity;
        results.currentIndex = index;
        selectedIdentity = identity;
        if (!same)
            backend.select(originalIndex(index));
    }
    function restoreSelection() {
        if (!selectedIdentity || retainingResults)
            return;
        for (let i = 0; i < viewItems.length; i++) {
            if (rowIdentity(viewItems[i]) === selectedIdentity) {
                results.currentIndex = i;
                // A reload drops the open details; ask for them again once
                // the rows are final, so the panel never waits forever.
                if (backend.details === "{}" && !backend.busy)
                    backend.select(originalIndex(i));
                return;
            }
        }
        results.currentIndex = -1;
        if (currentView !== "Search" || !items.some((row) => rowIdentity(row) === selectedIdentity))
            selectedIdentity = null;
    }
    // Whether the list's rows need more height than the page has. Updated
    // from signals rather than bound: the budget reads the page's laid-out
    // children, and a binding would loop through the layout.
    property bool listOverflowing: false
    function updateOverflow() {
        listOverflowing = shortResultsHeight() > detailsBudget();
    }
    onHeightChanged: Qt.callLater(updateOverflow)
    onCompactChanged: Qt.callLater(updateOverflow)
    onViewItemsChanged: {
        syncResults();
        updateOverflow();
        Qt.callLater(() => root.restoreSelection());
    }
    function propose(action) {
        if (!retainingResults) {
            if (action === "upgrade-all")
                markActiveRows(packageIdentities());
            else if (selected)
                markActiveRows([rowIdentity(selected)]);
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
        property string notificationHistory: "{}"
        property string lastBackgroundState: "{}"
        property int sidebarWidth: root.defaultSidebarWidth
        // Shown on the empty Search page until sources are checked again.
        property string searchHint: ""
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
        function onNotification_historyChanged() {
            preferences.notificationHistory = backend.notification_history;
        }
        function onBackground_stateChanged() {
            const state = JSON.parse(backend.background_state || "{}");
            if (state.last_check)
                preferences.lastBackgroundState = JSON.stringify({last_check: state.last_check, available: state.available, failures: state.failures || [], notify: false});
        }
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
                Qt.callLater(root.restoreSelection);
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
            if (root.liveItems.length > 0 && (!root.retainUntilDone || !backend.busy || root.liveItems.length >= root.retainedItems.length)) {
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
    // Search as you type. Short enough to feel live, long enough that each
    // keystroke does not start a query in every package manager.
    Timer {
        id: searchDebounce
        interval: 220
        onTriggered: root.submitSearch()
    }
    function submitSearch() {
        searchDebounce.stop();
        if (currentView !== "Search")
            return;
        queryDirty = false;
        sortColumn = "";
        reload();
    }
    Timer {
        id: completionHold
        interval: 1000
        onTriggered: root.completedRows = []
    }
    // Page switches fade in; controls never move while a page appears.
    NumberAnimation {
        id: pageSwitch
        target: pageContent
        property: "opacity"
        from: 0.35
        to: 1
        duration: root.revealDuration
        easing.type: Easing.OutCubic
    }
    onCurrentViewChanged: {
        Qt.callLater(updateOverflow);
        if (motionEnabled)
            pageSwitch.restart();
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
        // Polls only while background work can still deliver something.
        running: backend.busy || backend.needs_poll !== false
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
        backend.restoreNotificationHistory(preferences.notificationHistory);
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
            id: sidebar
            objectName: "sidebar"
            Layout.fillHeight: true
            Layout.preferredWidth: root.sidebarWidth
            // Switching between the rail and the full sidebar slides.
            Behavior on Layout.preferredWidth {
                enabled: railSwitch.running
                NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
            }
            color: root.surface
            clip: true
            Rectangle { anchors.right: parent.right; width: 1; height: parent.height; color: root.line }
            ColumnLayout {
                id: sidebarColumn
                anchors.fill: parent
                anchors.margins: root.sidebarRail ? 10 : 14
                spacing: 8
                RowLayout {
                    id: brandRow
                    spacing: 10
                    // Centered on the page title row beside it.
                    Layout.topMargin: Math.max(0, Math.round(pageContent.Layout.margins + root.controlHeight / 2
                        - sidebarColumn.anchors.margins - implicitHeight / 2))
                    Layout.leftMargin: root.sidebarRail ? 0 : 4
                    Layout.alignment: root.sidebarRail ? Qt.AlignHCenter : Qt.AlignLeft
                    Image {
                        objectName: "appLogo"
                        source: root.logoIconSource
                        sourceSize.width: 28
                        sourceSize.height: 28
                        fillMode: Image.PreserveAspectFit
                        Accessible.ignored: true
                    }
                    Controls.Label {
                        id: sidebarTitle
                        objectName: "sidebarTitle"
                        visible: !root.sidebarRail
                        text: "PkgDeck"
                        font.pointSize: root.font.pointSize * 1.55
                        font.weight: Font.Bold
                        color: root.ink
                    }
                }
                Item { Layout.preferredHeight: 18 }
                Item {
                    id: navigationList
                    Layout.fillWidth: true
                    implicitHeight: navigationColumn.implicitHeight
                    readonly property var pages: ["Search", "Installed", "Updates", "Clean", "Sources", "Settings"]
                    readonly property int currentPage: pages.indexOf(root.currentView)
                    // The selection pill slides between entries.
                    Rectangle {
                        objectName: "navigationHighlight"
                        readonly property Item target: navigationRepeater.count > 0 && navigationList.currentPage >= 0 ? navigationRepeater.itemAt(navigationList.currentPage) : null
                        visible: target !== null
                        width: parent.width
                        height: target ? target.height : 0
                        y: target ? target.y : 0
                        radius: root.controlRadius
                        color: root.selection
                        Behavior on y { NumberAnimation { duration: root.revealDuration; easing.type: Easing.OutCubic } }
                        Rectangle {
                            visible: !root.sidebarRail
                            width: 3
                            height: parent.height - 16
                            radius: 1.5
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.left: parent.left
                            anchors.leftMargin: 3
                            color: root.accent
                        }
                    }
                    Column {
                        id: navigationColumn
                        width: parent.width
                        spacing: 4
                        Repeater {
                            id: navigationRepeater
                            model: navigationList.pages
                            delegate: ActionButton {
                                required property string modelData
                                objectName: "navigation" + modelData
                                width: navigationColumn.width
                                // The rail shows icons only; the name moves to a tooltip.
                                text: root.sidebarRail ? "" : modelData
                                tooltipText: root.sidebarRail ? modelData : ""
                                Accessible.name: modelData
                                symbol: ({"Search":"search", "Installed":"installed", "Updates":"updates", "Clean":"remove", "Sources":"sources", "Settings":"settings"})[modelData]
                                navigation: true
                                centered: root.sidebarRail
                                current: root.currentView === modelData
                                // The sliding highlight draws the current entry.
                                background.opacity: current ? 0 : 1
                                onClicked: root.openView(modelData)
                            }
                        }
                    }
                }
                Item { Layout.fillHeight: true }
                RowLayout {
                    id: sidebarSignature
                    objectName: "signatureFooter"
                    visible: !root.sidebarRail
                    Layout.leftMargin: 4
                    spacing: 4
                    Controls.Label { objectName: "signaturePrefix"; text: "Made with"; color: root.muted; font.pointSize: root.font.pointSize * 0.9 }
                    DeckIcon { name: "heart"; ink: "#e34b5f"; Layout.preferredWidth: 14; Layout.preferredHeight: 14 }
                    Controls.Label { objectName: "signatureAuthor"; text: "by astro"; color: root.muted; font.pointSize: root.font.pointSize * 0.9 }
                }
            }
            // Drag the edge to resize. Dragging it narrow leaves only the
            // icons; double-click restores the default width. With keyboard
            // focus, Left and Right resize and Home restores it.
            MouseArea {
                id: sidebarResize
                objectName: "sidebarResize"
                visible: root.width >= 820
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                width: 12
                activeFocusOnTab: true
                Accessible.role: Accessible.Separator
                Accessible.name: "Resize sidebar"
                Keys.onPressed: (event) => {
                    const step = event.key === Qt.Key_Right ? 16 : event.key === Qt.Key_Left ? -16 : 0;
                    if (event.key === Qt.Key_Home)
                        preferences.sidebarWidth = root.defaultSidebarWidth;
                    else if (step !== 0) {
                        const next = root.sidebarWidth + step;
                        preferences.sidebarWidth = next < root.railThreshold ? root.railWidth
                            : Math.max(root.sidebarMinimumWidth, Math.min(root.sidebarMaximumWidth, next));
                    } else
                        return;
                    event.accepted = true;
                }
                hoverEnabled: true
                cursorShape: Qt.SplitHCursor
                property real pressX: 0
                property int startWidth: 0
                onPressed: (mouse) => {
                    pressX = mapToItem(root.contentItem, mouse.x, 0).x;
                    startWidth = root.sidebarWidth;
                }
                onPositionChanged: (mouse) => {
                    if (!pressed)
                        return;
                    const dragged = Math.round(startWidth + mapToItem(root.contentItem, mouse.x, 0).x - pressX);
                    preferences.sidebarWidth = dragged < root.railThreshold ? root.railWidth
                        : Math.max(root.sidebarMinimumWidth, Math.min(root.sidebarMaximumWidth, dragged));
                }
                onDoubleClicked: preferences.sidebarWidth = root.defaultSidebarWidth
                Rectangle {
                    anchors.right: parent.right
                    width: 2
                    height: parent.height
                    color: root.accent
                    opacity: sidebarResize.containsMouse || sidebarResize.pressed || sidebarResize.activeFocus ? 0.6 : 0
                    Behavior on opacity { NumberAnimation { duration: root.feedbackDuration } }
                }
            }
        }
        ColumnLayout {
            id: pageContent
            onImplicitHeightChanged: Qt.callLater(root.updateOverflow)
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.alignment: Qt.AlignTop
            Layout.margins: root.pageMargin
            spacing: 14
            RowLayout {
                Layout.fillWidth: true
                Kirigami.Heading {
                    objectName: "pageHeading"
                    text: root.currentView
                    elide: Text.ElideRight
                    color: root.ink
                    level: 1
                    font.pointSize: root.font.pointSize * 1.6
                    font.bold: true
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "addPackageButton"
                    // Installs a package from a file or link; it never adds a source.
                    text: root.headerIconsOnly ? "" : "Install from file…"
                    tooltipText: root.headerIconsOnly ? "Install from a file or link" : ""
                    symbol: "package"
                    visible: (root.currentView === "Search" || root.currentView === "Sources") && root.supportedFilePatterns().length > 0
                    enabled: !backend.writing
                    onClicked: { backend.checkSources(); root.rememberDialogFocus(); addPackageDialog.open(); }
                    Accessible.name: "Install from a file or link"
                }
                ActionButton {
                    objectName: "activityIndicator"
                    readonly property int pending: root.queuedCount + (backend.writing ? 1 : 0)
                    readonly property string label: backend.writing ? "Working" : "Activity"
                    text: root.headerIconsOnly ? "" : label
                    tooltipText: root.headerIconsOnly ? label : ""
                    symbol: "activity"
                    glyphColor: backend.writing ? root.accent : root.ink
                    Accessible.name: root.queuedCount > 0 ? "Activity, " + root.queuedCount + " queued" : backend.writing ? "Activity, working" : "Activity"
                    onClicked: root.activityOpen ? (root.activityOpen = false) : root.openView("Activity")
                    // Running and queued changes, as a badge on the button.
                    Rectangle {
                        objectName: "activityBadge"
                        visible: parent.pending > 0
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: -5
                        width: Math.max(height, badgeText.implicitWidth + 10)
                        height: 18
                        radius: 9
                        color: root.accent
                        scale: visible ? 1 : 0.5
                        Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutBack } }
                        Text {
                            id: badgeText
                            anchors.centerIn: parent
                            text: String(parent.parent.pending)
                            color: root.accentInk
                            font.pointSize: Theme.pointSize(Theme.captionScale)
                            font.bold: true
                        }
                    }
                }
                ActionButton {
                    objectName: "sourceFilter"
                    id: sourceFilterButton
                    visible: ["Search", "Installed", "Updates", "Clean"].indexOf(root.currentView) >= 0
                    readonly property string label: root.viewSourceFilters[root.currentView] ? root.sourceSummary() : "Filter sources"
                    text: root.headerIconsOnly ? "" : label
                    symbol: "filter"
                    glyphColor: root.viewSourceFilters[root.currentView] ? root.accent : root.ink
                    onClicked: root.toggleSourcePopup()
                    Accessible.name: "Filter this page by package source"
                    Layout.preferredWidth: root.headerIconsOnly ? implicitWidth : Math.min(240, Math.max(160, implicitWidth))
                    tooltipText: root.headerIconsOnly || root.viewSourceFilters[root.currentView] ? label : ""
                    Controls.Popup {
                        id: sourcePopup
                        objectName: "sourcePopup"
                        parent: Controls.Overlay.overlay
                        property var draftSources: []
                        property string searchText: ""
                        property bool showUnavailable: false
                        x: root.width - width - root.pageMargin
                        y: 76
                        width: Math.min(340, root.width - 32)
                        height: Math.min(root.height * 0.85, root.height - 100, implicitHeight)
                        padding: 10
                        modal: true
                        Controls.Overlay.modal: Rectangle { color: "transparent" }
                        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
                        transformOrigin: Controls.Popup.TopRight
                        enter: Transition {
                            ParallelAnimation {
                                NumberAnimation { property: "opacity"; from: 0; to: 1; duration: root.revealDuration; easing.type: Easing.OutCubic }
                                NumberAnimation { property: "scale"; from: root.motionEnabled ? 0.95 : 1; to: 1; duration: root.revealDuration; easing.type: Easing.OutCubic }
                            }
                        }
                        onAboutToShow: {
                            // Open just below the filter button, right-aligned with it.
                            const corner = sourceFilterButton.mapToItem(null, sourceFilterButton.width, sourceFilterButton.height);
                            x = Math.max(12, Math.min(root.width - width - 12, corner.x - width));
                            y = corner.y + 6;
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
                            ThemedTextField {
                                id: pickerSearch
                                objectName: "sourcePickerSearch"
                                Layout.fillWidth: true
                                placeholderText: "Find a source"
                                Accessible.name: "Find a source"
                                rightPadding: 40
                                onTextChanged: sourcePopup.searchText = text
                                ClearFieldButton {
                                    objectName: "clearSourceSearchButton"
                                    anchors.right: parent.right
                                    anchors.rightMargin: 4
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: pickerSearch.text.length > 0
                                    ink: root.muted
                                    hoverColor: root.line
                                    clearLabel: "Clear source search"
                                    onClicked: { pickerSearch.clear(); pickerSearch.forceActiveFocus(); }
                                }
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
                                        model: root.pickerSections()
                                        delegate: Column {
                                            required property var modelData
                                            required property int index
                                            readonly property string sourceId: modelData.id
                                            property alias checkBox: checkRow
                                            width: checklist.width
                                            readonly property bool usable: root.sourceInfo(sourceId).availability_kind === "available" && root.sourceSupportsView(sourceId)
                                            readonly property string section: modelData.section
                                            Controls.Label {
                                                width: parent.width
                                                topPadding: 8
                                                text: parent.section
                                                font.weight: Font.DemiBold
                                                font.pointSize: Theme.pointSize(Theme.captionScale)
                                                font.letterSpacing: 0.6
                                                font.capitalization: Font.AllUppercase
                                                leftPadding: 8
                                                color: root.muted
                                                visible: parent.modelData.showHeader
                                            }
                                            Controls.CheckDelegate {
                                                id: checkRow
                                                objectName: "sourceCheck-" + parent.sourceId
                                                width: parent.width
                                                text: root.sourceDisplayName(parent.sourceId)
                                                checked: sourcePopup.draftSources.indexOf(parent.sourceId) >= 0
                                                enabled: parent.usable && root.checkedSources().indexOf(parent.sourceId) >= 0 && (!checked || sourcePopup.draftSources.length > 1)
                                                onToggled: root.toggleDraftSource(parent.sourceId)
                                                // The label pads itself past the box; the row adds nothing.
                                                leftPadding: 0
                                                indicator: TickBox {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    anchors.left: parent.left
                                                    anchors.leftMargin: 8
                                                    ticked: checkRow.checked
                                                }
                                                background: Rectangle {
                                                    radius: root.controlRadius - 2
                                                    color: checkRow.hovered ? root.hoverTint : "transparent"
                                                    Behavior on color { ColorAnimation { duration: root.feedbackDuration } }
                                                }
                                                contentItem: Text {
                                                    objectName: "sourceCheckLabel"
                                                    text: checkRow.text
                                                    color: checkRow.enabled ? root.ink : root.muted
                                                    elide: Text.ElideRight
                                                    verticalAlignment: Text.AlignVCenter
                                                    leftPadding: 36
                                                }
                                            }
                                            Controls.Label {
                                                width: parent.width - 34
                                                x: 34
                                                text: !root.sourceSupportsView(parent.sourceId) ? "Not supported in this view" : (root.sourceInfo(parent.sourceId).summary || "Unavailable")
                                                color: root.muted
                                                wrapMode: Text.WordWrap
                                                font.pointSize: Theme.pointSize(Theme.smallScale)
                                                visible: !parent.usable
                                            }
                                        }
                                    }
                                }
                                Controls.ScrollBar.vertical: DeckScrollBar { ink: root.muted }
                            }
                            ActionButton {
                                objectName: "unavailableSourceToggle"
                                Layout.fillWidth: true
                                text: sourcePopup.showUnavailable ? "Hide unavailable" : "Show unavailable"
                                symbol: sourcePopup.showUnavailable ? "up" : "down"
                                flat: true
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
                                    symbol: "installed"
                                    primary: true
                                    enabled: sourcePopup.draftSources.length > 0
                                    onClicked: root.applySourceDraft()
                                }
                            }
                        }
                        background: Rectangle {
                            color: root.surface
                            radius: root.cardRadius
                            border.color: root.line
                        }
                    }
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
                    // Enter searches at once; typing searches after a short pause.
                    onSubmitted: root.submitSearch()
                    onDownRequested: {
                        results.forceActiveFocus();
                        if (root.viewItems.length > 0)
                            root.choose(0);
                    }
                    onQueryEdited: {
                        root.queryDirty = true;
                        if (searchPane.text.trim().length === 0)
                            root.submitSearch();
                        else
                            searchDebounce.restart();
                    }
                }
            }
            // Empty Search: a quiet centered prompt, like the other empty states.
            Item {
                id: searchEmptyState
                objectName: "searchEmptyState"
                visible: root.currentView === "Search" && searchPane.text.trim().length === 0 && !root.openingInput
                    && root.viewItems.length === 0
                Layout.fillWidth: true
                Layout.fillHeight: true
                Column {
                    anchors.centerIn: parent
                    // Slightly above center reads as centered in the page.
                    anchors.verticalCenterOffset: -parent.height * 0.1
                    width: Math.min(parent.width - 32, 420)
                    spacing: 8
                    DeckIcon {
                        name: "search"
                        ink: root.muted
                        width: 34
                        height: 34
                        anchors.horizontalCenter: parent.horizontalCenter
                    }
                    Controls.Label {
                        text: "Find apps and packages"
                        color: root.ink
                        font.weight: Font.DemiBold
                        width: parent.width
                        horizontalAlignment: Text.AlignHCenter
                    }
                    Controls.Label {
                        id: searchHint
                        objectName: "searchHint"
                        text: root.searchHint()
                        color: root.muted
                        width: parent.width
                        wrapMode: Text.WordWrap
                        horizontalAlignment: Text.AlignHCenter
                    }
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
            ActionProgress {
                objectName: "operationProgress"
                cancelable: true
                onCancelRequested: backend.cancel()
                visible: backend.writing && !!root.actionProgress.label && (!root.activityOpen ||
                    !root.activityRows.some(entry => entry.id === root.actionProgress.activity_id && (entry.state === "running" || entry.state === "authorizing")))
                Layout.fillWidth: true
                label: root.actionProgress.label || ""
                done: root.actionProgress.done || 0
                total: root.actionProgress.total || 1
                transferred: root.actionProgress.transferred || 0
                transferTotal: root.actionProgress.transfer_total || 0
                ink: root.ink
                muted: root.muted
                accent: root.accent
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
                    leftPadding: 42
                    rightPadding: 42
                    background: Rectangle {
                        color: root.surface
                        radius: root.controlRadius + 1
                        border.color: installedFilterField.activeFocus ? root.accent : root.line
                        border.width: installedFilterField.activeFocus ? 2 : 1
                        Behavior on border.color { ColorAnimation { duration: root.feedbackDuration } }
                        DeckIcon {
                            name: "filter"
                            ink: installedFilterField.activeFocus ? root.accent : root.muted
                            anchors.left: parent.left
                            anchors.leftMargin: 14
                            anchors.verticalCenter: parent.verticalCenter
                            width: 17
                            height: 17
                        }
                    }
                    // Large lists filter once typing pauses.
                    onTextChanged: {
                        if (root.items.length > 400)
                            installedFilterDebounce.restart();
                        else
                            root.installedFilter = text;
                    }
                    Timer {
                        id: installedFilterDebounce
                        interval: 150
                        onTriggered: root.installedFilter = installedFilterField.text
                    }
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
                    ClearFieldButton {
                        objectName: "clearInstalledFilterButton"
                        anchors.right: parent.right
                        anchors.rightMargin: 5
                        anchors.verticalCenter: parent.verticalCenter
                        visible: installedFilterField.text.length > 0
                        ink: root.muted
                        hoverColor: root.line
                        clearLabel: "Clear installed filter"
                        onClicked: { installedFilterField.clear(); installedFilterField.forceActiveFocus(); }
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
            SettingsPage {
                id: settingsScroll
                app: root
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                // The scrollbar sits beside the cards, not at the window edge.
                Layout.maximumWidth: cardWidth + 24
                Layout.fillHeight: true
            }
            // The outcome of the last change. Failures stay until dismissed;
            // success fades on its own.
            Rectangle {
                id: noticeBanner
                objectName: "changeNotice"
                readonly property var notice: JSON.parse(backend.notice || "{}")
                readonly property color toneColor: notice.kind === "error" ? root.danger : notice.kind === "success" ? root.success : root.muted
                // Successes show as a toast; the banner is for problems.
                readonly property bool shown: notice.title !== undefined && notice.kind !== "success" && notice.kind !== "info" && root.currentView !== "Settings"
                visible: shown || opacity > 0
                Layout.fillWidth: true
                implicitHeight: noticeRow.implicitHeight + 18
                radius: root.controlRadius + 1
                color: root.tint(toneColor, root.dark ? 0.12 : 0.08)
                border.color: root.tint(toneColor, 0.35)
                opacity: shown ? 1 : 0
                Behavior on opacity { NumberAnimation { duration: root.revealDuration } }
                onNoticeChanged: {
                    if (notice.title !== undefined && (notice.kind === "success" || notice.kind === "info"))
                        changeToast.show(notice.title, root.undoAction(notice) ? "Undo" : "", notice.kind);
                }
                RowLayout {
                    id: noticeRow
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 8
                    spacing: 10
                    DeckIcon {
                        name: noticeBanner.notice.kind === "error" ? "warning" : "installed"
                        ink: noticeBanner.toneColor
                        Layout.preferredWidth: 18
                        Layout.preferredHeight: 18
                        Layout.alignment: Qt.AlignTop
                        Layout.topMargin: 2
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 2
                        Controls.Label {
                            objectName: "changeNoticeTitle"
                            text: noticeBanner.notice.title || ""
                            textFormat: Text.PlainText
                            color: root.ink
                            font.weight: Font.DemiBold
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                        Controls.Label {
                            objectName: "changeNoticeDetail"
                            text: noticeBanner.notice.detail || ""
                            textFormat: Text.PlainText
                            visible: text.length > 0
                            color: root.muted
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }
                    ActionButton {
                        objectName: "changeNoticeSettings"
                        visible: noticeBanner.notice.action === "settings" && root.systemAuthorizationSupported
                        text: "Settings"
                        symbol: "settings"
                        flat: true
                        onClicked: root.openView("Settings")
                    }
                    ClearFieldButton {
                        objectName: "dismissChangeNotice"
                        ink: root.muted
                        hoverColor: root.line
                        clearLabel: "Dismiss"
                        Layout.alignment: Qt.AlignTop
                        onClicked: backend.dismissNotice()
                    }
                }
            }
            Rectangle {
                visible: root.readFailures.length > 0 && root.viewItems.length > 0 &&
                    ["Search", "Installed", "Updates", "Clean", "Sources"].indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                implicitHeight: failureBanner.implicitHeight + 16
                radius: root.controlRadius + 1
                color: root.tint(root.warning, root.dark ? 0.12 : 0.1)
                border.color: root.tint(root.warning, 0.35)
                RowLayout {
                    id: failureBanner
                    anchors.fill: parent
                    anchors.leftMargin: 12
                    anchors.rightMargin: 8
                    spacing: 10
                    DeckIcon { name: "warning"; ink: root.warning; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
                    Controls.Label {
                        objectName: "sourceFailureNotice"
                        text: root.sourceFailureTitle() + (root.currentView === "Updates" ? ". Update all will retry the check." : "")
                        color: root.ink
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    ActionButton {
                        objectName: "sourceFailureRetryNotice"
                        text: "Retry"
                        symbol: "refresh"
                        flat: true
                        enabled: !backend.busy && !(root.currentView === "Search" && root.queryDirty)
                        onClicked: root.reload(true)
                    }
                    ActionButton {
                        objectName: "turnOffFailedSource"
                        visible: root.readFailures.length === 1 && root.canTurnOff(root.readFailures[0].source)
                        text: root.readFailures.length === 1 ? "Turn off " + root.sourceDisplayName(root.readFailures[0].source) : ""
                        symbol: "cancel"
                        flat: true
                        onClicked: root.setManagerEnabled(root.readFailures[0].source, false)
                    }
                    ActionButton {
                        objectName: "sourceFailureDetails"
                        text: "Details"
                        symbol: "help"
                        flat: true
                        onClicked: { root.rememberDialogFocus(); sourceFailuresDialog.open(); }
                    }
                }
            }
            Rectangle {
                id: resultsBox
                objectName: "resultsBox"
                Layout.fillWidth: true
                // Lists take their content's height up to the space the page
                // has, then scroll. The first load fills the page for its
                // placeholder rows; a running change never resizes the list.
                readonly property bool loadingEmpty: backend.busy && !backend.writing && !root.openingInput && root.viewItems.length === 0
                readonly property bool overflowing: root.listOverflowing
                Layout.fillHeight: loadingEmpty || overflowing
                Layout.preferredHeight: root.viewItems.length === 0 && !backend.busy ? 150
                    : detailsPanel.visible ? Math.min(root.shortResultsHeight(), root.height * (root.compact ? 0.24 : 0.42),
                        root.detailsBudget() - detailsPanel.Layout.preferredHeight)
                    : Math.min(root.shortResultsHeight(), root.detailsBudget())
                Layout.minimumHeight: detailsPanel.visible ? root.detailsListHeight() : 130
                visible: root.currentView === root.resultView && root.currentView !== "Settings" &&
                    (root.currentView !== "Search" || root.viewItems.length > 0 || root.readFailures.length > 0 ||
                        (backend.busy && !root.openingInput) || searchPane.text.trim().length > 0)
                color: root.surface
                radius: root.cardRadius
                border.color: results.activeFocus ? root.accent : root.line
                Behavior on border.color { ColorAnimation { duration: root.feedbackDuration } }
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 1
                    spacing: 0
                    RowLayout {
                        id: resultsHeadingRow
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
                        DeckIcon {
                            id: resultsSpinner
                            objectName: "resultsBusy"
                            readonly property bool running: ((backend.busy && !backend.writing) || !!backend.refreshing) && root.motionEnabled
                            visible: running && results.count > 0
                            name: "refresh"
                            ink: root.accent
                            Layout.preferredWidth: 16
                            Layout.preferredHeight: 16
                            RotationAnimation on rotation {
                                running: resultsSpinner.visible
                                from: 0
                                to: 360
                                duration: 1100
                                loops: Animation.Infinite
                            }
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
                        ActionButton {
                            objectName: "resultsCancel"
                            text: "Cancel"
                            symbol: "cancel"
                            // A running change cancels from its progress line or row.
                            visible: backend.busy && !backend.writing
                            implicitHeight: 30
                            onClicked: backend.cancel()
                        }
                        ActionButton {
                            objectName: "reloadButton"
                            visible: !backend.busy || backend.writing
                            text: ""
                            symbol: "refresh"
                            flat: true
                            glyphColor: root.muted
                            implicitHeight: 30
                            Accessible.name: "Reload"
                            tooltipText: "Reload (Ctrl+R)"
                            enabled: !backend.busy || backend.writing
                            onClicked: root.reload(true)
                        }
                    }
                    Rectangle { Layout.fillWidth: true; height: 1; color: root.line; visible: results.count > 0 }
                    RowLayout {
                        id: columnHeaderRow
                        visible: !root.compact && results.count > 0
                        spacing: 12
                        Layout.fillWidth: true
                        Layout.leftMargin: 16
                        Layout.rightMargin: 16
                        Layout.topMargin: 10
                        Layout.bottomMargin: 10
                        Item { visible: root.currentView === "Updates"; Layout.preferredWidth: 28 }
                        Item { Layout.preferredWidth: root.iconSlotSize; visible: root.pageWidth >= 420 }
                        Controls.Label {
                            objectName: "columnHeader0"
                            text: (root.currentView === "Sources" ? "SOURCE" : "NAME / SOURCE") + root.sortArrow("name")
                            color: root.muted
                            font.pointSize: Theme.pointSize(Theme.captionScale)
                            font.weight: Font.DemiBold
                            font.letterSpacing: 0.6
                            font.underline: sortNameArea.activeFocus
                            elide: Text.ElideRight
                            Layout.preferredWidth: root.nameWidth
                            Layout.fillWidth: root.currentView === "Sources" || root.medium
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
                                // Shift+Left/Right resizes the column from the keyboard.
                                Keys.onPressed: (event) => {
                                    if (!(event.modifiers & Qt.ShiftModifier) || (event.key !== Qt.Key_Left && event.key !== Qt.Key_Right))
                                        return;
                                    root.nameWidth = Math.max(80, Math.min(600, root.nameWidth + (event.key === Qt.Key_Right ? 16 : -16)));
                                    event.accepted = true;
                                }
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
                            font.pointSize: Theme.pointSize(Theme.captionScale)
                            font.weight: Font.DemiBold
                            font.letterSpacing: 0.6
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
                                // Shift+Left/Right resizes the column from the keyboard.
                                Keys.onPressed: (event) => {
                                    if (!(event.modifiers & Qt.ShiftModifier) || (event.key !== Qt.Key_Left && event.key !== Qt.Key_Right))
                                        return;
                                    root.versionWidth = Math.max(80, Math.min(600, root.versionWidth + (event.key === Qt.Key_Right ? 16 : -16)));
                                    event.accepted = true;
                                }
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
                            visible: root.currentView !== "Sources" && !root.medium
                            text: (root.currentView === "Sources" ? "CAPABILITIES" : "SUMMARY") + root.sortArrow(root.currentView === "Sources" ? "capabilities" : "summary")
                            color: root.muted
                            font.pointSize: Theme.pointSize(Theme.captionScale)
                            font.weight: Font.DemiBold
                            font.letterSpacing: 0.6
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
                        model: resultsModel
                        clip: true
                        reuseItems: true
                        add: Transition {
                            enabled: root.motionEnabled && root.animateListChanges
                            NumberAnimation { property: "opacity"; from: 0; to: 1; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                        }
                        remove: Transition {
                            enabled: root.motionEnabled && root.animateListChanges
                            NumberAnimation { property: "opacity"; to: 0; duration: Theme.feedbackDuration; easing.type: Easing.InCubic }
                        }
                        displaced: Transition {
                            enabled: root.motionEnabled && root.animateListChanges
                            NumberAnimation { properties: "x,y"; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                        }
                        move: Transition {
                            enabled: root.motionEnabled && root.animateListChanges
                            NumberAnimation { properties: "x,y"; duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                        }
                        enabled: !root.retainingResults
                        currentIndex: -1
                        onCountChanged: {
                            if (!root.selectedIdentity)
                                currentIndex = -1;
                        }
                        // Opening details shrinks the list; keep the open row in view.
                        onHeightChanged: if (currentIndex >= 0) Qt.callLater(() => { if (results.currentIndex >= 0) results.positionViewAtIndex(results.currentIndex, ListView.Contain); })
                        keyNavigationEnabled: false
                        activeFocusOnTab: true
                        Controls.ScrollBar.vertical: DeckScrollBar { ink: root.muted }
                        Keys.onDownPressed: { root.keyboardNavigation = true; root.choose(Math.min(count - 1, currentIndex + 1)); }
                        Keys.onUpPressed: { root.keyboardNavigation = true; root.choose(Math.max(0, currentIndex - 1)); }
                        Keys.onPressed: (event) => {
                            root.keyboardNavigation = true;
                            if (event.key === Qt.Key_PageDown)
                                root.choose(Math.min(count - 1, (currentIndex < 0 ? 0 : currentIndex) + 10));
                            else if (event.key === Qt.Key_PageUp)
                                root.choose(Math.max(0, (currentIndex < 0 ? 0 : currentIndex) - 10));
                            else if (event.key === Qt.Key_Home)
                                root.choose(0);
                            else if (event.key === Qt.Key_End)
                                root.choose(count - 1);
                            else if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && currentIndex >= 0)
                                root.runRowAction(currentIndex);
                            else
                                return;
                            event.accepted = true;
                        }
                        delegate: Controls.ItemDelegate {
                            id: packageRow
                            required property string rowJson
                            required property int index
                            readonly property var modelData: JSON.parse(rowJson)
                            readonly property string identity: root.rowIdentity(modelData)
                            // This row is being changed right now.
                            readonly property bool active: root.rowIsActive(identity)
                            readonly property string rowAction: root.rowActionName(modelData)
                            readonly property bool packageKind: modelData.kind === "package"
                            width: Math.max(0, ListView.view.width - Theme.scrollGutter)
                            height: root.rowHeight(modelData)
                            topPadding: modelData.groupStart ? root.groupHeaderHeight : 0
                            leftPadding: 16
                            rightPadding: 12
                            leftInset: 0
                            rightInset: 0
                            topInset: 0
                            bottomInset: 0
                            highlighted: results.currentIndex === index
                            enabled: true
                            Accessible.name: (packageKind ? (modelData.update === "available" ? "Update available. " : (root.isInstalled(modelData) ? "Installed. " : "Not installed. ")) : "")
                                + (modelData.kind === "source" ? root.sourceDisplayName(modelData.source) : (modelData.display_name || modelData.name) + ", " + root.sourceLine(modelData))
                                + (modelData.summary ? ", " + modelData.summary : "")
                            onClicked: { root.keyboardNavigation = false; results.forceActiveFocus(); root.choose(index); }
                            Rectangle {
                                visible: !!packageRow.modelData.groupStart
                                anchors.top: parent.top
                                anchors.left: parent.left
                                anchors.right: parent.right
                                height: root.groupHeaderHeight
                                color: root.selection
                                z: 2
                                Rectangle { width: 3; height: parent.height; color: root.accent }
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.leftMargin: 16
                                    anchors.rightMargin: 16
                                    Controls.Label {
                                        objectName: packageRow.modelData.groupStart ? "packageGroupTitle" : ""
                                        text: packageRow.modelData.groupTitle || ""
                                        color: root.ink
                                        font.bold: true
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        text: (packageRow.modelData.groupSources || []).join(" · ")
                                        color: root.accent
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        elide: Text.ElideRight
                                        Layout.maximumWidth: packageRow.width * 0.5
                                    }
                                }
                            }
                            background: Item {
                                Rectangle { anchors.bottom: parent.bottom; anchors.left: parent.left; anchors.right: parent.right; anchors.leftMargin: 12; anchors.rightMargin: 12; height: 1; color: root.line; opacity: 0.6 }
                                Rectangle {
                                    anchors.fill: parent
                                    anchors.topMargin: packageRow.topPadding + 2
                                    anchors.bottomMargin: 2
                                    anchors.leftMargin: 6
                                    radius: root.controlRadius
                                    Behavior on color { ColorAnimation { duration: root.feedbackDuration } }
                                    color: packageRow.highlighted ? root.selection : (packageRow.hovered ? root.hoverTint : "transparent")
                                    border.width: 1
                                    border.color: packageRow.visualFocus || (results.activeFocus && packageRow.highlighted && root.keyboardNavigation) ? root.accent : "transparent"
                                    Rectangle {
                                        anchors.fill: parent
                                        radius: parent.radius
                                        color: root.success
                                        opacity: root.completedRows.indexOf(packageRow.identity) >= 0 ? 0.18 : 0
                                        Behavior on opacity { NumberAnimation { duration: root.motionEnabled ? 240 : 0 } }
                                    }
                                    // Progress of the change running on this row.
                                    RowProgress {
                                        objectName: "rowProgress"
                                        anchors.left: parent.left
                                        anchors.right: parent.right
                                        anchors.bottom: parent.bottom
                                        anchors.leftMargin: 10
                                        anchors.rightMargin: 10
                                        anchors.bottomMargin: 3
                                        running: packageRow.active
                                        value: packageRow.active ? root.actionFraction() : -1
                                    }
                                }
                            }
                            contentItem: RowLayout {
                                spacing: 12
                                Controls.CheckBox {
                                    id: packageCheck
                                    visible: root.currentView === "Updates" && packageRow.packageKind
                                    checked: root.packageChecked(packageRow.modelData)
                                    enabled: true
                                    onToggled: root.togglePackage(packageRow.modelData)
                                    Accessible.name: "Select " + (packageRow.modelData.display_name || packageRow.modelData.name || "")
                                    Layout.preferredWidth: 28
                                    Layout.alignment: Qt.AlignVCenter
                                    indicator: TickBox {
                                        anchors.centerIn: parent
                                        ticked: packageCheck.checked
                                    }
                                    contentItem: Item {}
                                }
                                // The app's own icon when it has one, with its
                                // source as a small badge; otherwise the source icon.
                                Item {
                                    objectName: "rowIconSlot"
                                    // The narrowest cards keep their width for the name.
                                    visible: root.pageWidth >= 420
                                    Layout.preferredWidth: root.iconSlotSize
                                    Layout.preferredHeight: root.iconSlotSize
                                    Layout.alignment: root.compact ? Qt.AlignTop : Qt.AlignVCenter
                                    Layout.topMargin: root.compact ? 12 : 0
                                    Rectangle {
                                        anchors.fill: parent
                                        radius: Theme.controlRadius
                                        color: root.hoverTint
                                        visible: rowIcon.status !== Image.Ready
                                    }
                                    DeckIcon {
                                        objectName: "packageIconFallback"
                                        visible: rowIcon.status !== Image.Ready
                                        anchors.centerIn: parent
                                        width: Math.round(parent.width * 0.55)
                                        height: width
                                        name: packageRow.modelData.kind === "cleanup" ? "remove" : packageRow.modelData.source
                                        ink: root.muted
                                    }
                                    Image {
                                        id: rowIcon
                                        objectName: "packageIcon"
                                        anchors.fill: parent
                                        visible: status === Image.Ready
                                        asynchronous: true
                                        source: root.iconUrl(packageRow.modelData.icon || "")
                                        sourceSize.width: Math.ceil(width * Screen.devicePixelRatio)
                                        sourceSize.height: Math.ceil(height * Screen.devicePixelRatio)
                                        fillMode: Image.PreserveAspectFit
                                        opacity: status === Image.Ready ? 1 : 0
                                        Behavior on opacity { NumberAnimation { duration: Theme.revealDuration } }
                                        Accessible.ignored: true
                                    }
                                    Rectangle {
                                        objectName: "sourceBadge"
                                        visible: rowIcon.status === Image.Ready
                                        width: 16
                                        height: 16
                                        radius: 8
                                        x: parent.width - width + 4
                                        y: parent.height - height + 4
                                        color: root.surface
                                        border.color: root.line
                                        DeckIcon {
                                            anchors.centerIn: parent
                                            width: 11
                                            height: 11
                                            name: packageRow.modelData.source
                                            ink: root.muted
                                        }
                                    }
                                }
                                ColumnLayout {
                                    spacing: 3
                                    Layout.preferredWidth: root.compact || root.medium ? -1 : root.nameWidth
                                    Layout.fillWidth: root.compact || root.medium || packageRow.modelData.kind === "source"
                                    Layout.alignment: Qt.AlignVCenter
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 8
                                        Controls.Label {
                                            objectName: "packageName"
                                            text: packageRow.modelData.kind === "source" ? root.sourceDisplayName(packageRow.modelData.source) : (packageRow.modelData.display_name || packageRow.modelData.name)
                                            color: root.ink
                                            font.bold: true
                                            textFormat: Text.PlainText
                                            elide: Text.ElideRight
                                            // The chip, when shown, keeps to the column's end so it
                                            // lines up from row to row.
                                            Layout.fillWidth: true
                                        }
                                        Controls.Label {
                                            objectName: "installedChip"
                                            visible: packageRow.packageKind && root.currentView === "Search" && root.isInstalled(packageRow.modelData)
                                            text: "Installed"
                                            color: root.success
                                            font.pointSize: Theme.pointSize(Theme.captionScale)
                                            font.weight: Font.DemiBold
                                            leftPadding: 7
                                            rightPadding: 7
                                            topPadding: 1
                                            bottomPadding: 1
                                            background: Rectangle { radius: height / 2; color: root.tint(root.success, 0.12) }
                                        }
                                    }
                                    Controls.Label {
                                        objectName: "packageSourceLine"
                                        visible: packageRow.modelData.kind !== "source"
                                        text: root.sourceLine(packageRow.modelData)
                                        color: root.muted
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        objectName: "compactVersion"
                                        visible: root.compact && (packageRow.modelData.kind !== "source" || root.showUnavailableSources)
                                        text: root.versionText(packageRow.modelData)
                                        font.family: "monospace"
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        color: packageRow.modelData.kind === "failure" ? root.danger : (packageRow.modelData.update === "available" ? root.accent : root.muted)
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        visible: root.compact && packageRow.modelData.kind !== "source"
                                        text: packageRow.modelData.summary || ""
                                        color: root.muted
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                }
                                ColumnLayout {
                                    visible: !root.compact && (packageRow.modelData.kind !== "source" || root.showUnavailableSources)
                                    // Fixed width keeps every summary aligned, elided or not.
                                    Layout.preferredWidth: root.versionWidth
                                    Layout.minimumWidth: root.versionWidth
                                    Layout.maximumWidth: root.versionWidth
                                    spacing: 3
                                    // Updates show the new version, with the installed one below.
                                    readonly property bool upgrade: packageRow.modelData.update === "available" && !!packageRow.modelData.installed && !!packageRow.modelData.candidate && packageRow.modelData.installed !== packageRow.modelData.candidate
                                    Controls.Label {
                                        objectName: "wideVersion"
                                        text: parent.upgrade ? packageRow.modelData.candidate : root.versionText(packageRow.modelData)
                                        font.family: "monospace"
                                        color: packageRow.modelData.kind === "failure" ? root.danger : (packageRow.modelData.update === "available" ? root.accent : root.muted)
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        Layout.fillWidth: true
                                    }
                                    Controls.Label {
                                        visible: parent.upgrade
                                        text: "from " + (packageRow.modelData.installed || "")
                                        font.family: "monospace"
                                        color: root.muted
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                        font.pointSize: Theme.pointSize(Theme.captionScale)
                                        Layout.fillWidth: true
                                    }
                                }
                                Controls.Label {
                                    objectName: "rowSummary"
                                    visible: !root.compact && !root.medium && packageRow.modelData.kind !== "source"
                                    Layout.fillWidth: true
                                    // Long text must not squeeze the fixed columns.
                                    Layout.preferredWidth: 0
                                    text: packageRow.modelData.summary || ""
                                    color: root.muted
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                }
                                Controls.CheckBox {
                                    id: managerEnabled
                                    objectName: "managerEnabled"
                                    visible: root.currentView === "Sources" && packageRow.modelData.kind === "source" && packageRow.modelData.available
                                    text: checked ? "Enabled" : "Disabled"
                                    checked: root.checkedSources().indexOf(packageRow.modelData.source) >= 0
                                    enabled: !backend.writing && (!checked || root.checkedSources().length > 1)
                                    Accessible.name: (checked ? "Disable " : "Enable ") + root.sourceDisplayName(packageRow.modelData.source)
                                    Controls.ToolTip.visible: hovered && root.compact
                                    Controls.ToolTip.text: Accessible.name
                                    onClicked: root.setManagerEnabled(packageRow.modelData.source, checked)
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
                                    visible: packageRow.packageKind && root.containerSource(packageRow.modelData.source) && root.isInstalled(packageRow.modelData) && !!packageRow.modelData.reference && !packageRow.active
                                    enabled: (!backend.busy || backend.writing) && !root.retainingResults
                                    text: ""
                                    symbol: "updates"
                                    flat: true
                                    glyphColor: enabled ? root.accent : root.muted
                                    Accessible.name: "Pull " + (packageRow.modelData.display_name || packageRow.modelData.name) + " from " + root.sourceLine(packageRow.modelData)
                                    tooltipText: Accessible.name
                                    Layout.preferredWidth: 38
                                    horizontalPadding: 8
                                    opacity: 1
                                    onClicked: { root.markActiveRows([packageRow.identity]); backend.propose("upgrade", root.originalIndex(packageRow.index)); }
                                }
                                ActionButton {
                                    objectName: "rowPackageAction"
                                    visible: packageRow.rowAction.length > 0
                                    enabled: packageRow.active || ((!backend.busy || backend.writing) && !root.retainingResults)
                                    readonly property string verb: ({install: "Install ", remove: "Remove ", upgrade: "Update ", clean: "Run cleanup "})[packageRow.rowAction] || ""
                                    text: ""
                                    symbol: packageRow.active ? "cancel" : packageRow.rowAction === "upgrade" ? "updates" : packageRow.rowAction === "install" ? "install" : "remove"
                                    flat: true
                                    glyphColor: !enabled ? root.muted : packageRow.active ? root.muted : packageRow.rowAction === "upgrade" ? root.accent : packageRow.rowAction === "install" ? root.success : root.danger
                                    Accessible.name: packageRow.active ? "Cancel " + (backend.status || "the change")
                                        : verb + (packageRow.modelData.display_name || packageRow.modelData.name) + (packageRow.modelData.kind === "cleanup" ? "" : " from " + root.sourceLine(packageRow.modelData))
                                    tooltipText: Accessible.name
                                    Layout.preferredWidth: 38
                                    horizontalPadding: 8
                                    opacity: 1
                                    onClicked: packageRow.active ? backend.cancel() : root.runRowAction(packageRow.index)
                                }
                            }
                        }
                        // Placeholder rows while the first results load, so the
                        // list keeps its shape instead of flashing empty.
                        Column {
                            objectName: "loadingPlaceholder"
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: 16
                            spacing: 22
                            visible: results.count === 0 && backend.busy && !backend.writing
                            opacity: 0.55
                            SequentialAnimation on opacity {
                                running: results.count === 0 && backend.busy && root.motionEnabled
                                loops: Animation.Infinite
                                NumberAnimation { to: 1; duration: 700; easing.type: Easing.InOutSine }
                                NumberAnimation { to: 0.55; duration: 700; easing.type: Easing.InOutSine }
                            }
                            Repeater {
                                model: Math.max(1, Math.min(8, Math.floor((resultsBox.height - 60) / 58)))
                                delegate: RowLayout {
                                    required property int index
                                    width: parent.width
                                    spacing: 14
                                    ColumnLayout {
                                        spacing: 7
                                        Layout.preferredWidth: root.compact ? parent.width * 0.6 : root.nameWidth
                                        Rectangle { radius: 4; color: root.hoverTint; Layout.preferredHeight: 11; Layout.preferredWidth: parent.width * (0.55 + (index * 37 % 40) / 100) }
                                        Rectangle { radius: 4; color: root.hoverTint; Layout.preferredHeight: 9; Layout.preferredWidth: parent.width * 0.35 }
                                    }
                                    Rectangle { visible: !root.compact; radius: 4; color: root.hoverTint; Layout.preferredHeight: 10; Layout.preferredWidth: root.versionWidth * 0.6 }
                                    Rectangle { visible: !root.compact; radius: 4; color: root.hoverTint; Layout.preferredHeight: 10; Layout.fillWidth: true; Layout.rightMargin: 60 + (index * 53 % 120) }
                                }
                            }
                        }
                        Column {
                            anchors.centerIn: parent
                            width: parent.width - 32
                            spacing: 10
                            visible: results.count === 0
                            DeckIcon {
                                id: emptyStateIcon
                                objectName: "emptyStateIcon"
                                readonly property string message: root.emptyStateMessage()
                                readonly property bool goodNews: /up to date$|^Nothing to clean/.test(message)
                                readonly property bool failed: root.readFailures.length > 0 && !goodNews
                                name: failed ? "warning"
                                    : goodNews ? "installed"
                                    : root.currentView === "Search" ? "search" : "package"
                                ink: failed ? root.warning
                                    : goodNews ? root.success : root.muted
                                visible: !backend.busy && message.length > 0
                                anchors.horizontalCenter: parent.horizontalCenter
                                width: 34
                                height: 34
                            }
                            Controls.Label {
                                objectName: "emptyState"
                                width: parent.width
                                horizontalAlignment: Text.AlignHCenter
                                wrapMode: Text.WordWrap
                                // Titled like the Search page's empty state.
                                color: root.ink
                                font.weight: Font.DemiBold
                                visible: text.length > 0
                                text: root.emptyStateMessage()
                            }
                            Controls.Label {
                                objectName: "sourceFailureEmptyHint"
                                width: parent.width
                                horizontalAlignment: Text.AlignHCenter
                                wrapMode: Text.WordWrap
                                visible: (root.readFailures.length > 0 || text.length > 0) && !backend.busy
                                text: root.readFailures.length > 0
                                    ? (emptyStateIcon.goodNews ? root.sourceFailureTitle() + ". " : "") + (root.currentView === "Updates" ? "Retry to check for updates." : "Retry to check again.")
                                    : (root.currentView === "Updates" || root.currentView === "Clean") && results.count === 0 ? root.checkedAgo() : ""
                                color: root.readFailures.length > 0 && emptyStateIcon.goodNews ? root.warning : root.muted
                            }
                            Row {
                                anchors.horizontalCenter: parent.horizontalCenter
                                spacing: 8
                                visible: root.readFailures.length > 0 && !backend.busy
                                ActionButton {
                                    objectName: "sourceFailureRetry"
                                    text: root.readFailures.length === 1 ? "Retry" : "Reload"
                                    symbol: "refresh"
                                    onClicked: root.readFailures.length === 1
                                        ? root.retryFailedSource(root.readFailures[0].source) : root.reload(true)
                                }
                                ActionButton {
                                    objectName: "sourceFailureEmptyDetails"
                                    text: "Details"
                                    symbol: "help"
                                    onClicked: { root.rememberDialogFocus(); sourceFailuresDialog.open(); }
                                }
                            }
                            ActionButton {
                                objectName: "loadAgainButton"
                                anchors.horizontalCenter: parent.horizontalCenter
                                visible: root.loadStopped && results.count === 0
                                text: root.currentView === "Search" ? "Search again" : "Load again"
                                symbol: "refresh"
                                onClicked: root.reload(true)
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
                // Opens with the selection and stays open while the next row's
                // details load (the panel shows a placeholder meanwhile).
                visible: root.selected !== null && root.listViews.indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                // Never taller than the page leaves after the list's minimum, so
                // short windows keep the actions below on screen; the panel
                // scrolls its own content instead.
                Layout.preferredHeight: Math.max(root.detailsMinimumHeight(),
                    Math.min(root.height * (root.compact ? 0.32 : 0.48), detailsPanel.idealHeight,
                        root.detailsBudget() - root.detailsListHeight()))
                Layout.minimumHeight: root.detailsMinimumHeight()
                selected: root.selected
                selectionIdentity: root.rowIdentity(root.selected)
                sourceName: root.sourceDisplayName
                installed: root.selected !== null && root.selected.kind === "package" && root.isInstalled(root.selected)
                readonly property string rowAction: root.rowActionName(root.selected)
                actionText: ({install: "Install", remove: "Remove", upgrade: "Update", clean: "Clean"})[rowAction] || ""
                actionSymbol: rowAction === "upgrade" ? "updates" : rowAction === "install" ? "install" : "remove"
                actionTone: rowAction === "upgrade" ? "accent" : rowAction === "install" ? "success" : "danger"
                actionEnabled: (!backend.busy || backend.writing) && !root.retainingResults
                onActionRequested: root.runRowAction(results.currentIndex)
                Behavior on Layout.preferredHeight {
                    enabled: detailsPanel.visible
                    NumberAnimation { duration: Theme.layoutDuration; easing.type: Easing.OutCubic }
                }
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
                    narrow: root.pageWidth < 360
                    busy: backend.busy && !backend.writing
                    writing: false
                    selectedCount: root.selectedCount()
                    uncheckedCount: root.uncheckedPackages.length
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
                    visible: root.currentView === "Sources" && root.repositorySourcesAvailable
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
            }
            Item {
                objectName: "pageFiller"
                visible: root.listViews.indexOf(root.currentView) >= 0 && !resultsBox.Layout.fillHeight && !searchEmptyState.visible
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
        // Sized to its repositories, scrolling only past most of the window.
        height: Math.min(root.height * 0.85, Math.max(260, 200 + repositoryList.contentHeight + repositoryExtras.implicitHeight))
        title: "Repositories"
        modal: true
        standardButtons: Controls.Dialog.Close
        onAboutToShow: backend.loadRepositories()
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 12
            Flow {
                Layout.fillWidth: true
                spacing: 8
                ActionButton {
                    objectName: "addFlatpakRepositoryButton"
                    text: "Add Flatpak repository"; symbol: "install"
                    visible: root.repositoryFeatures.flatpak_user || root.repositoryFeatures.flatpak_system || false
                    enabled: !backend.busy
                    onClicked: { root.rememberDialogFocus(); addRepositoryDialog.open(); }
                }
                ActionButton {
                    objectName: "editAptSourcesButton"
                    text: "Edit APT sources"; symbol: "settings"
                    visible: root.repositoryFeatures.apt_editor || false
                    enabled: !backend.busy
                    onClicked: root.repositoryChange({backend: "apt", name: "sources", scope: "system"}, "open_editor")
                }
                ActionButton { text: ""; symbol: "refresh"; Accessible.name: "Reload repositories"; tooltipText: Accessible.name; enabled: !backend.busy; onClicked: backend.loadRepositories() }
            }
            Controls.BusyIndicator { visible: backend.busy; running: visible; Layout.alignment: Qt.AlignHCenter }
            ListView {
                id: repositoryList
                objectName: "repositoryList"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 8
                model: root.repositoryReport.repositories || []
                Controls.ScrollBar.vertical: DeckScrollBar { ink: root.muted }
                delegate: Rectangle {
                    required property var modelData
                    width: ListView.view.width - 14
                    height: repositoryCard.implicitHeight + 20
                    color: root.canvas
                    radius: root.controlRadius + 1
                    border.color: root.line
                    GridLayout {
                        id: repositoryCard
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 10
                        columns: repositoriesDialog.width < 620 ? 1 : 2
                        columnSpacing: 12
                        rowSpacing: 4
                        RowLayout {
                            Layout.fillWidth: true
                            spacing: 6
                            Controls.CheckBox {
                                objectName: "repositoryEnabled"
                                visible: root.repositoryEditable(modelData)
                                checked: modelData.enabled
                                enabled: !backend.busy
                                Accessible.name: "Enable " + (modelData.title || modelData.name)
                                onClicked: {
                                    root.repositoryChange(modelData, "set_enabled", {enabled: checked});
                                    checked = Qt.binding(() => modelData.enabled);
                                }
                            }
                            Controls.Label {
                                visible: !root.repositoryEditable(modelData)
                                text: modelData.enabled ? "Enabled" : "Disabled"
                                color: root.muted
                                font.pointSize: root.font.pointSize * 0.9
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
                                    text: root.sourceDisplayName(modelData.backend) + " · " + (modelData.scope === "system" ? "System" : "User")
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
                                text: ""; symbol: "up"; visible: modelData.backend === "flatpak" && root.repositoryEditable(modelData)
                                Accessible.name: "Increase repository priority"; enabled: !backend.busy && modelData.priority < 9999
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority + 1})
                            }
                            ActionButton {
                                text: ""; symbol: "down"; visible: modelData.backend === "flatpak" && root.repositoryEditable(modelData)
                                Accessible.name: "Decrease repository priority"; enabled: !backend.busy && modelData.priority > 0
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority - 1})
                            }
                            ActionButton {
                                objectName: "removeRepositoryButton"
                                text: ""; symbol: "remove"; visible: modelData.backend === "flatpak" && root.repositoryEditable(modelData)
                                Accessible.name: "Remove " + modelData.name; enabled: !backend.busy
                                tooltipText: Accessible.name
                                onClicked: root.repositoryChange(modelData, "remove")
                            }
                        }
                    }
                }
            }
            Controls.Label { id: repositoryExtras; Layout.fillWidth: true; color: root.muted; textFormat: Text.PlainText; wrapMode: Text.WordWrap; text: (root.repositoryReport.errors || []).join("\n"); visible: text.length > 0 }
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
        onAboutToShow: {
            repositoryName.text = "";
            repositoryUrl.text = "";
            standardButton(Controls.Dialog.Ok).enabled = Qt.binding(() => addRepositoryDialog.validName && addRepositoryDialog.validUrl);
        }
        onOpened: repositoryName.forceActiveFocus()
        onClosed: root.restoreDialogFocus()
        onAccepted: root.repositoryChange({backend: "flatpak", name: repositoryName.text.trim(), scope: repositoryScope.currentText === "System" ? "system" : "user"}, "add", {url: repositoryUrl.text.trim()})
        contentItem: ColumnLayout {
            ThemedTextField { id: repositoryName; objectName: "repositoryName"; placeholderText: "Name"; Accessible.name: "Repository name"; Layout.fillWidth: true }
            Controls.Label {
                objectName: "repositoryNameError"
                text: "Use letters, numbers, dots, dashes, or underscores. Do not start with a dash."
                visible: repositoryName.text.length > 0 && !addRepositoryDialog.validName
                color: root.danger
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            ThemedTextField { id: repositoryUrl; objectName: "repositoryUrl"; placeholderText: "https://…/repository.flatpakrepo"; Accessible.name: "Repository URL"; Layout.fillWidth: true }
            Controls.Label {
                objectName: "repositoryUrlError"
                text: "Enter an HTTPS .flatpakrepo URL."
                visible: repositoryUrl.text.length > 0 && !addRepositoryDialog.validUrl
                color: root.danger
                wrapMode: Text.WordWrap
                Layout.fillWidth: true
            }
            ThemedComboBox {
                id: repositoryScope
                objectName: "repositoryScope"
                model: [root.repositoryFeatures.flatpak_user ? "User" : "", root.repositoryFeatures.flatpak_system ? "System" : ""].filter(Boolean)
                Accessible.name: "Installation scope"
                Layout.fillWidth: true
            }
        }
    }
    ThemedDialog {
        id: sourceFailuresDialog
        objectName: "sourceFailuresDialog"
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 620)
        modal: true
        title: "Source checks"
        standardButtons: Controls.Dialog.Close
        onClosed: root.restoreDialogFocus()
        contentItem: ColumnLayout {
            spacing: 10
            // Sized to its reasons, scrolling only when they outgrow the window.
            DeckScrollView {
                ink: root.muted
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(failureList.implicitHeight, Math.max(120, root.height - 260))
                contentWidth: availableWidth
                clip: true
                Column {
                    id: failureList
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
                                    // A source that keeps failing can be switched off here.
                                    visible: root.canTurnOff(modelData.source)
                                    text: "Turn off"
                                    symbol: "cancel"
                                    flat: true
                                    Accessible.name: "Turn off " + root.sourceDisplayName(modelData.source)
                                    onClicked: { root.setManagerEnabled(modelData.source, false); sourceFailuresDialog.close(); }
                                }
                                ActionButton {
                                    text: "Retry"
                                    symbol: "refresh"
                                    enabled: !backend.busy && modelData.kind !== "unsupported" && !(root.currentView === "Search" && root.queryDirty)
                                    onClicked: root.retryFailedSource(modelData.source)
                                }
                            }
                            Controls.Label {
                                objectName: "sourceFailureReason"
                                width: parent.width
                                text: root.failureSummary(modelData.source)
                                textFormat: Text.PlainText
                                wrapMode: Text.WordWrap
                                color: root.muted
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
        contentItem: Item {
            Image {
                id: fullScreenshot
                anchors.fill: parent
                source: screenshotDialog.visible ? root.screenshotUrl : ""
                asynchronous: true
                cache: true
                // Decoded at the size shown, sharp on HiDPI screens.
                sourceSize.width: Math.ceil(screenshotDialog.width * Screen.devicePixelRatio)
                sourceSize.height: Math.ceil(screenshotDialog.height * Screen.devicePixelRatio)
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
        height: Math.min(root.height - 32, Math.max(230, confirmationBody.implicitHeight + 150))
        title: "Confirm changes"
        modal: true
        readonly property var preview: JSON.parse(backend.confirmation_data || "{}")
        readonly property string summaryText: preview.summary || preview.body || backend.confirmation
        readonly property var summaryLines: summaryText.split("\n")
        property bool detailsExpanded: false
        standardButtons: Controls.Dialog.NoButton
        onClosed: root.restoreDialogFocus()
        onAboutToShow: detailsExpanded = false
        onOpened: confirmationCancel.forceActiveFocus()
        onAccepted: backend.confirm(true)
        onRejected: { root.activeRows = []; backend.confirm(false); }
        // The apply key is Alt plus the first letter of the action that
        // Cancel's Alt+C does not take ("Clean" applies with Alt+L).
        // Ctrl+Enter always applies and Escape always cancels.
        readonly property string applyWord: (preview.action || "Apply").trim().split(/\s+/)[0]
        readonly property int applyMnemonic: {
            for (let i = 0; i < applyWord.length; i++) {
                if (/[a-bd-z]/i.test(applyWord.charAt(i)))
                    return i;
            }
            return -1;
        }
        Shortcut {
            sequence: "Alt+" + confirmation.applyWord.charAt(confirmation.applyMnemonic).toUpperCase()
            enabled: confirmation.visible && confirmation.applyMnemonic >= 0
            onActivated: confirmation.accept()
        }
        Shortcut {
            sequences: ["Ctrl+Return", "Ctrl+Enter"]
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
                    text: confirmation.applyWord
                    mnemonicIndex: confirmation.applyMnemonic
                    tooltipText: confirmation.applyMnemonic >= 0 ? "Alt+" + text.charAt(mnemonicIndex).toUpperCase() + " or Ctrl+Enter" : "Ctrl+Enter"
                    symbol: ""
                    primary: true
                    // Removing reads as destructive, not as the default go-ahead.
                    primaryColor: confirmation.applyWord === "Remove" ? Theme.danger : Theme.accent
                    primaryInk: confirmation.applyWord === "Remove" ? (Theme.dark ? "#0e1015" : "#ffffff") : Theme.accentInk
                    Layout.minimumWidth: 80
                    onClicked: confirmation.accept()
                }
                ActionButton {
                    id: confirmationCancel
                    objectName: "confirmationCancel"
                    text: "Cancel"
                    mnemonicIndex: 0
                    tooltipText: "Alt+C or Esc"
                    symbol: ""
                    Layout.minimumWidth: 80
                    onClicked: confirmation.reject()
                }
            }
        }
        contentItem: DeckScrollView {
            ink: root.muted
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
                RowLayout {
                    visible: !!confirmation.preview.flatpak_ref_scope
                    Layout.fillWidth: true
                    Controls.Label { text: "Install for"; color: root.muted }
                    ThemedComboBox {
                        objectName: "flatpakRefScope"
                        Layout.fillWidth: true
                        model: ["User", "System"]
                        currentIndex: confirmation.preview.flatpak_ref_scope === "system" ? 1 : 0
                        Accessible.name: "Installation scope"
                        onActivated: backend.setOpenFlatpakScope(currentIndex === 1)
                    }
                }
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: summaryContent.implicitHeight + 28
                    color: root.selection
                    radius: root.controlRadius + 1
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
                    symbol: confirmation.detailsExpanded ? "up" : "down"
                    flat: true
                    onClicked: confirmation.detailsExpanded = !confirmation.detailsExpanded
                }
                Rectangle {
                    visible: confirmation.detailsExpanded && !!confirmation.preview.details
                    Layout.fillWidth: true
                    implicitHeight: confirmationDetails.implicitHeight + 28
                    color: root.canvas
                    radius: root.controlRadius + 1
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
    // Short-lived confirmation of a finished change, with Undo when the
    // change has a safe inverse (install and remove).
    function undoAction(notice) {
        return notice && notice.undo === true && (notice.undo_action === "install" || notice.undo_action === "remove") && notice.target
            ? {action: notice.undo_action, package: notice.target} : null;
    }
    function undoLastChange() {
        const undo = undoAction(JSON.parse(backend.notice || "{}"));
        if (!undo)
            return;
        const identity = rowIdentity(undo.package);
        const index = items.findIndex((row) => rowIdentity(row) === identity);
        if (index < 0)
            return;
        markActiveRows([identity]);
        backend.propose(undo.action, index);
    }
    Toast {
        id: changeToast
        objectName: "changeToast"
        parent: root.contentItem
        z: 30
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: 24
        width: Math.min(460, parent.width - 32)
        onActionTriggered: root.undoLastChange()
        onDismissed: if (backend.notice && backend.notice !== "{}") backend.dismissNotice()
    }
    // The Activity drawer, over any page.
    Rectangle {
        objectName: "activityScrim"
        parent: root.contentItem
        anchors.fill: parent
        z: 20
        color: Qt.rgba(0, 0, 0, root.dark ? 0.45 : 0.22)
        opacity: root.activityOpen ? 1 : 0
        visible: opacity > 0
        Behavior on opacity { NumberAnimation { duration: Theme.revealDuration } }
        MouseArea { anchors.fill: parent; onClicked: root.activityOpen = false }
    }
    Rectangle {
        id: activityDrawer
        objectName: "activityDrawer"
        parent: root.contentItem
        z: 21
        width: Math.min(480, parent.width - (root.width < 520 ? 0 : 48))
        height: parent.height
        x: root.activityOpen ? parent.width - width : parent.width
        visible: root.activityOpen || x < parent.width
        color: root.surface
        border.color: root.line
        Behavior on x { NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic } }
        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 20
            spacing: 14
            RowLayout {
                Layout.fillWidth: true
                Kirigami.Heading {
                    objectName: "activityHeading"
                    text: "Activity"
                    color: root.ink
                    level: 1
                    font.pointSize: Theme.pointSize(Theme.titleScale)
                    font.bold: true
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "closeActivity"
                    text: ""
                    symbol: "cancel"
                    flat: true
                    tooltipText: "Close (Esc)"
                    Accessible.name: "Close activity"
                    onClicked: root.activityOpen = false
                }
            }
            ActivityPane {
                Layout.fillWidth: true
                Layout.fillHeight: true
                showHeader: false
                entries: root.activityRows
                progress: root.actionProgress
                sourceName: root.sourceDisplayName
                canCancelRunning: backend.writing
                textFont: root.font
                onCancelQueued: backend.cancelQueued()
                onCancelRunning: backend.cancel()
            }
        }
    }
    Shortcut {
        sequence: "Ctrl+J"
        onActivated: root.activityOpen ? (root.activityOpen = false) : root.openView("Activity")
    }
    Shortcut {
        sequence: "Escape"
        enabled: root.activityOpen
        onActivated: root.activityOpen = false
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
                searchPane.focusSearch(true, Qt.ShortcutFocusReason);
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
        enabled: root.listViews.indexOf(root.currentView) >= 0
        onActivated: root.reload(true)
    }
    Shortcut {
        sequences: ["Ctrl+,", "Ctrl+6"]
        onActivated: root.openView("Settings")
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
        enabled: root.currentView === "Updates" && (!backend.busy || backend.writing) && root.selectedCount() > 0
        onActivated: root.upgradeUpdates()
    }
    Shortcut {
        sequence: "Ctrl+M"
        enabled: !backend.busy || backend.writing
        onActivated: root.propose("refresh")
    }
    // Escape never cancels running work (the visible Cancel button does).
    // In the search field it clears the query; in the list it closes details.
    Shortcut {
        sequence: "Escape"
        enabled: !root.activityOpen && !!root.activeFocusItem && ((root.activeFocusItem.objectName === "searchField" && searchPane.text.length > 0)
            || (root.activeFocusItem === results && root.selected !== null))
        onActivated: {
            if (root.activeFocusItem === results)
                detailsPanel.closeRequested();
            else
                searchPane.text = "";
        }
    }
}
