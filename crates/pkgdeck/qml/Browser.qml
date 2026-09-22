import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtCore
import org.kde.kirigami as Kirigami

Controls.ApplicationWindow {
    id: root
    required property var backend
    readonly property var repositoryReport: JSON.parse(backend.repositories || "{}")
    function repositoryChange(row, action, extra) {
        const request = Object.assign({backend: row.backend, name: row.name, scope: row.scope, action: action}, extra || {});
        backend.changeRepository(JSON.stringify(request));
    }
    property string currentView: "Search"
    property string resultView: "Search"
    property var liveItems: JSON.parse(backend.rows || "[]")
    property var retainedItems: []
    property bool retainingResults: false
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
    property string screenshotUrl: ""
    property string screenshotCaption: ""
    property bool closePending: false
    property bool queryDirty: false
    property var selectedIdentity: null
    function rowIdentity(row) {
        return row ? JSON.stringify([row.source, row.name, row.architecture, row.remote || null, row.scope, row.reference || null]) : "";
    }
    property var selected: !retainingResults && results.currentIndex >= 0 && results.currentIndex < viewItems.length ? viewItems[results.currentIndex] : null
    // Explicitly checked source ids, comma-joined; empty means every
    // available source. Unchecking hides a source from all queries, which
    // is how backends you never use stay silent.
    property string sourceSelection: ""
    property string installedFilter: ""
    // Session-only filter showing just the apps installed from more than
    // one source. Session-only like the text filter: view state, not a
    // persisted preference.
    property bool multiSourceOnly: false
    property bool useSudo: argument("--auth", preferences.authorization) === "sudo"
    function updateOnly(source) {
        return ["fwupd", "codex", "claude", "grok", "opencode"].indexOf(source) >= 0;
    }
    readonly property var sourceIds: ["apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem", "fwupd", "codex", "claude", "grok", "opencode"]
    readonly property var sourceNames: ["APT", "DNF", "Pacman", "Zypper", "Snap", "Homebrew", "AppImage", "Flatpak", "Cargo", "npm", "pnpm", "Bun", "pip", "pipx", "uv", "Composer", "RubyGems", "Firmware", "Codex (standalone)", "Claude Code (standalone)", "Grok (standalone)", "OpenCode (standalone)"]
    function checkedSources() {
        if (sourceSelection === "")
            return sourceIds.slice();
        const checked = sourceSelection.split(",").filter((id) => sourceIds.indexOf(id) >= 0);
        return checked.length > 0 ? checked : sourceIds.slice();
    }
    function checkedCsv() {
        const checked = checkedSources();
        return checked.length >= sourceIds.length ? "" : checked.join(",");
    }
    function sourceSummary() {
        const checked = checkedSources();
        if (checked.length >= sourceIds.length)
            return "All sources";
        if (checked.length === 1)
            return sourceNames[sourceIds.indexOf(checked[0])];
        return checked.length + " sources";
    }
    function toggleSource(id) {
        let checked = checkedSources();
        const at = checked.indexOf(id);
        if (at >= 0) {
            if (checked.length <= 1)
                return;
            checked.splice(at, 1);
        } else {
            checked.push(id);
            checked.sort((a, b) => sourceIds.indexOf(a) - sourceIds.indexOf(b));
        }
        sourceSelection = checked.length >= sourceIds.length ? "" : checked.join(",");
        preferences.sourceList = sourceSelection;
        preferences.source = "";
        if (["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0)
            root.reload();
    }
    // Delegate lookup by position in sourceIds. Popup content reparents to
    // the Overlay, so findChild cannot reach the checkboxes; the Repeater
    // hands out the live delegate for real clicks in tests.
    function sourceCheckAt(index) {
        return checklistRepeater.itemAt(index);
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
    property var viewItems: {
        let rows = root.currentView === "Search" ? items.filter((row) => !isFabricated(row)) : items.slice();
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
        } else if (root.currentView === "Search") {
            const query = search.text.trim().toLowerCase();
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
    readonly property bool compact: width < 760
    readonly property bool dark: preferences.appearance === 1 || (preferences.appearance === 0 && Qt.styleHints.colorScheme === Qt.Dark)
    readonly property color canvas: dark ? "#000000" : "#f3f5f8"
    readonly property color surface: dark ? "#101014" : "#ffffff"
    readonly property color ink: dark ? "#ecf1f8" : "#1c2b3e"
    readonly property color muted: dark ? "#a2b1c4" : "#57677e"
    readonly property color line: dark ? "#2a2e37" : "#dce3ec"
    readonly property color accent: dark ? "#80b6ff" : "#245fc6"
    readonly property color selection: dark ? "#1a2740" : "#e8f0ff"
    color: canvas
    font.family: "sans-serif"
    font.pixelSize: 14
    palette.window: canvas
    palette.base: surface
    palette.text: ink
    palette.windowText: ink
    palette.buttonText: ink
    palette.button: surface
    palette.highlight: accent
    palette.highlightedText: dark ? "#111820" : "#ffffff"

    component ActionButton: Controls.Button {
        id: control
        property color glyphColor: root.ink
        property bool primary: false
        property bool navigation: false
        property string symbol: "package"
        Accessible.name: text
        implicitHeight: 38
        horizontalPadding: 16
        verticalPadding: 4
        opacity: enabled ? 1 : 0.45
        scale: down ? 0.98 : 1
        Behavior on opacity { NumberAnimation { duration: root.feedbackDuration } }
        Behavior on scale { NumberAnimation { duration: root.feedbackDuration; easing.type: Easing.OutCubic } }
        background: Rectangle {
            radius: 7
            Behavior on color { ColorAnimation { duration: root.feedbackDuration } }
            color: control.primary && control.enabled ? root.accent : (control.hovered ? root.selection : root.surface)
            border.color: control.activeFocus ? root.accent : (control.navigation ? "transparent" : root.line)
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
                    ink: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : control.glyphColor
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                    Layout.alignment: Qt.AlignVCenter
                }
                Text {
                    text: control.text
                    visible: text.length > 0
                    font: control.font
                    color: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : root.ink
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

    component ThemedComboBox: Controls.ComboBox {
        id: combo
        implicitHeight: 38
        background: Rectangle {
            color: root.surface
            radius: 8
            border.color: combo.activeFocus ? root.accent : root.line
            border.width: combo.activeFocus ? 2 : 1
        }
        contentItem: Text {
            text: combo.displayText
            color: combo.enabled ? root.ink : root.muted
            elide: Text.ElideRight
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
        const at = sourceIds.indexOf(id);
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

    property url repositoryIconSource: root.dark ? "qrc:/pkgdeck/github-dark.png" : "qrc:/pkgdeck/github.png"
    property url logoIconSource: "qrc:/pkgdeck/logo.svg"
    readonly property url repositoryUrl: "https://github.com/astrovm/PkgDeck"
    width: 1100
    height: 760
    minimumWidth: 360
    minimumHeight: 400
    visible: true
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
        if (backend.writing && ["Search", "Installed", "Updates", "Sources"].indexOf(view) >= 0)
            return;
        queryDirty = false;
        selectedIdentity = null;
        uncheckedPackages = [];
        if (currentView !== view) {
            retainingResults = false;
            retainedItems = [];
            revealedRows = new Set();
            completedRows = [];
        }
        currentView = view;
        results.currentIndex = -1;
        if (view === "Search")
            search.forceActiveFocus();
        else if (["Search", "Installed", "Updates", "Sources"].indexOf(view) >= 0)
            reload();
    }
    // Reload the current view. Without force, a cached snapshot serves
    // instantly with no worker; force always queries native managers.
    function reload(force) {
        retainedItems = currentView === resultView ? items.slice() : [];
        retainingResults = retainedItems.length > 0;
        revealedRows = new Set(retainedItems.map(rowIdentity));
        resultView = currentView;
        results.currentIndex = -1;
        selectedIdentity = null;
        uncheckedPackages = [];
        // Installed filtering is client-side over the loaded rows (see
        // viewItems), so the backend always returns the full installed set
        // and typing never triggers a native query.
        backend.load(currentView, currentView === "Installed" ? "" : search.text, checkedCsv(), useSudo, force === true);
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
    function propose(action) {
        if (!retainingResults)
            backend.propose(action, originalIndex(results.currentIndex));
    }
    function focusResultsAfterLoad() {
        if (!queryDirty && !installedFilterField.activeFocus && !sourcePopup.opened && !confirmation.opened
                && ["Search", "Installed", "Updates", "Sources"].indexOf(currentView) >= 0)
            results.forceActiveFocus();
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
    }
    onClosing: function (close) {
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
            if (!backend.writing && ["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0)
                postWriteReload.restart();
        }
        function onRowsChanged() {
            if (root.liveItems.length > 0) {
                root.retainingResults = false;
                if (!backend.writing)
                    root.markChangedRows();
            }
            // Streaming partials re-sort rows around the selection: follow
            // the selected identity instead of the row index.
            results.currentIndex = -1;
            if (root.selectedIdentity) {
                for (let i = 0; i < root.viewItems.length; i++) {
                    if (root.rowIdentity(root.viewItems[i]) === root.selectedIdentity) {
                        results.currentIndex = i;
                        break;
                    }
                }
            }
            if (root.viewItems.length > 0)
                root.focusResultsAfterLoad();
        }
        function onConfirmationChanged() {
            if (backend.confirmation.length)
                confirmation.open();
            else
                confirmation.close();
        }
    }
    Timer {
        id: completionHold
        interval: 1000
        onTriggered: root.completedRows = []
    }
    NumberAnimation {
        id: detailsReveal
        target: detailsContent
        property: "opacity"
        from: 0.55
        to: 1
        duration: root.revealDuration
        easing.type: Easing.OutCubic
    }
    Timer {
        interval: 40
        running: backend.busy
        repeat: true
        onTriggered: backend.poll()
    }
    Timer {
        id: postWriteReload
        interval: 0
        onTriggered: {
            if (!backend.busy && !root.closePending)
                root.reload(true);
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
    }

    RowLayout {
        anchors.fill: parent
        spacing: 0
        Rectangle {
            Layout.fillHeight: true
            Layout.preferredWidth: 196
            visible: !root.compact
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
                        font.pixelSize: 25
                        font.bold: true
                        color: root.ink
                    }
                }
                Item { Layout.preferredHeight: 24 }
                Repeater {
                    model: ["Search", "Installed", "Updates", "Sources", "Settings", "About"]
                    delegate: ActionButton {
                        required property string modelData
                        Layout.fillWidth: true
                        text: modelData
                        symbol: ({"Search":"search", "Installed":"installed", "Updates":"updates", "Sources":"sources", "Settings":"settings", "About":"help"})[modelData]
                        enabled: !backend.writing
                        navigation: true
                        primary: root.currentView === modelData
                        onClicked: root.openView(modelData)
                    }
                }
                Item { Layout.fillHeight: true }
                RowLayout {
                    Layout.fillWidth: true
                    spacing: 4
                    Controls.Label { text: "Made with"; color: root.muted; font.pixelSize: 11 }
                    DeckIcon { name: "heart"; ink: "#e34b5f"; Layout.preferredWidth: 14; Layout.preferredHeight: 14 }
                    Controls.Label { text: "by astro"; color: root.muted; font.pixelSize: 11 }
                    Controls.ToolButton {
                        objectName: "repositoryLink"
                        implicitWidth: 28
                        implicitHeight: 28
                        Accessible.name: "Open PkgDeck on GitHub"
                        onClicked: Qt.openUrlExternally(root.repositoryUrl)
                        background: Rectangle {
                            radius: 5
                            color: parent.hovered ? root.selection : "transparent"
                            border.color: parent.activeFocus ? root.accent : "transparent"
                        }
                        contentItem: Image {
                            objectName: "repositoryIcon"
                            source: root.repositoryIconSource
                            sourceSize.width: 18
                            sourceSize.height: 18
                            fillMode: Image.PreserveAspectFit
                            Accessible.ignored: true
                        }
                    }
                }

            }
        }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.margins: root.compact ? 12 : 28
            spacing: 14
            RowLayout {
                Layout.fillWidth: true
                ThemedComboBox {
                    visible: root.compact
                    model: ["Search", "Installed", "Updates", "Sources", "Settings", "About"]
                    currentIndex: model.indexOf(root.currentView)
                    enabled: !backend.writing
                    onActivated: root.openView(currentText)
                    Accessible.name: "Navigation"
                    Layout.fillWidth: true
                }
                Kirigami.Heading {
                    visible: !root.compact
                    text: root.currentView
                    color: root.ink
                    level: 1
                    font.pointSize: 21
                    font.bold: true
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "sourceFilter"
                    id: sourceFilterButton
                    visible: ["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0
                    text: root.sourceSummary()
                    symbol: "sources"
                    enabled: !backend.writing
                    onClicked: sourcePopup.open()
                    Accessible.name: "Package source filter"
                    Layout.preferredWidth: 210
                    Controls.Popup {
                        id: sourcePopup
                        objectName: "sourcePopup"
                        y: sourceFilterButton.height + 4
                        width: 250
                        height: 340
                        padding: 4
                        closePolicy: Controls.Popup.CloseOnEscape | Controls.Popup.CloseOnPressOutside
                        contentItem: Flickable {
                            anchors.fill: parent
                            clip: true
                            contentWidth: width
                            contentHeight: checklist.height
                            Column {
                                id: checklist
                                width: parent.width
                                Repeater {
                                    id: checklistRepeater
                                    model: root.sourceIds
                                    delegate: Controls.CheckDelegate {
                                        id: checkRow
                                        required property var modelData
                                        required property int index
                                        objectName: "sourceCheck-" + modelData
                                        width: checklist.width
                                        text: root.sourceNames[index]
                                        checked: root.checkedSources().indexOf(modelData) >= 0
                                        enabled: !checked || root.checkedSources().length > 1
                                        onToggled: root.toggleSource(modelData)
                                        indicator: TickBox {
                                            anchors.verticalCenter: parent.verticalCenter
                                            anchors.left: parent.left
                                            anchors.leftMargin: 8
                                            ticked: checkRow.checked
                                        }
                                        background: Rectangle {
                                            color: checkRow.hovered ? root.selection : "transparent"
                                        }
                                        contentItem: Text {
                                            text: root.sourceNames[index]
                                            color: checkRow.enabled ? root.ink : root.muted
                                            elide: Text.ElideRight
                                            verticalAlignment: Text.AlignVCenter
                                            leftPadding: 34
                                        }
                                    }
                                }
                            }
                            Controls.ScrollBar.vertical: Controls.ScrollBar { }
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
                Layout.fillWidth: true
                visible: root.currentView === "Search"
                Controls.TextField {
                    id: search
                    objectName: "searchField"
                    Layout.fillWidth: true
                    placeholderText: "Search apps and packages"
                    Accessible.name: "Search packages"
                    enabled: !backend.writing
                    selectByMouse: true
                    implicitHeight: 44
                    color: root.ink
                    placeholderTextColor: root.muted
                    leftPadding: 14
                    background: Rectangle {
                        color: root.surface
                        radius: 8
                        border.color: search.activeFocus ? root.accent : root.line
                        border.width: search.activeFocus ? 2 : 1
                    }
                    onAccepted: {
                        root.currentView = "Search";
                        root.queryDirty = false;
                        // A fresh search resets to best-match order: a stale
                        // column sort would otherwise silently win over it.
                        root.sortColumn = "";
                        root.reload(true);
                    }
                    Keys.onDownPressed: {
                        results.forceActiveFocus();
                        if (root.viewItems.length > 0)
                            root.choose(0);
                    }
                    onTextChanged: root.queryDirty = true
                }
                ActionButton {
                    text: "Search"
                    symbol: "search"
                    primary: true
                    enabled: !backend.writing && search.text.trim().length > 0
                    onClicked: { root.currentView = "Search"; root.queryDirty = false; root.sortColumn = ""; root.reload(true); }
                }
            }
            RowLayout {
                Layout.fillWidth: true
                visible: root.currentView === "Installed"
                Controls.TextField {
                    id: installedFilterField
                    objectName: "installedFilterField"
                    Layout.fillWidth: true
                    text: root.installedFilter
                    placeholderText: "Filter installed packages"
                    Accessible.name: "Filter installed packages"
                    enabled: !backend.writing
                    selectByMouse: true
                    implicitHeight: 44
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
                    text: "Duplicate installs"
                    checked: root.multiSourceOnly
                    enabled: !backend.writing
                    onToggled: root.multiSourceOnly = checked
                    Accessible.name: "Show only packages installed from multiple sources"
                    Layout.alignment: Qt.AlignVCenter
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
            ColumnLayout {
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                Layout.fillHeight: true
                Controls.Label { text: "Appearance" }
                ThemedComboBox {
                    objectName: "appearanceSetting"
                    model: ["System", "Dark", "Light"]
                    currentIndex: preferences.appearance
                    onActivated: preferences.appearance = currentIndex
                    Accessible.name: "Appearance"
                    Layout.fillWidth: true
                    Layout.maximumWidth: 420
                }
                Controls.CheckBox {
                    objectName: "animationsSetting"
                    text: "Animations"
                    checked: !root.reduceMotion
                    onToggled: root.reduceMotion = !checked
                    Accessible.name: "Enable interface animations"
                }
                Controls.Label {
                    text: "Authentication"
                }
                ThemedComboBox {
                    objectName: "authorizationSetting"
                    model: ["Host polkit agent", "Existing sudo credentials"]
                    currentIndex: root.useSudo ? 1 : 0
                    onActivated: {
                        root.useSudo = currentIndex === 1;
                        preferences.authorization = root.useSudo ? "sudo" : "polkit";
                    }
                    Accessible.name: "Authentication"
                    Layout.fillWidth: true
                    Layout.maximumWidth: 420
                }
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: "Polkit opens your system authentication dialog. Sudo uses an existing authorization. Homebrew and development tools run without elevation."
                }
                // Absorbs leftover height so the settings stack stays top-anchored.
                Item { Layout.fillHeight: true }
            }
            Controls.ScrollView {
                id: aboutScroll
                objectName: "aboutScroll"
                visible: root.currentView === "About"
                Layout.fillWidth: true
                Layout.fillHeight: true
                contentWidth: availableWidth
                clip: true
                Controls.Label {
                    objectName: "aboutText"
                    width: aboutScroll.availableWidth
                    verticalAlignment: Text.AlignTop
                    wrapMode: Text.WordWrap
                    text: "PkgDeck " + backend.version + "\nA unified package interface for Linux.\n\nKeyboard shortcuts\nCtrl+1: Search • Ctrl+2: Installed • Ctrl+3: Updates • Ctrl+4: Sources\nCtrl+F: search • Ctrl+L: focus results • Up/Down: select • Ctrl+I: install • Ctrl+D: remove • Ctrl+U: update • Ctrl+M: refresh source • Ctrl+R: reload\nEscape: cancel current work\n\nRefresh sources checks package metadata. Updating apps changes installed packages. Changes require confirmation."
                    textFormat: Text.PlainText
                }
            }
            Rectangle {
                id: resultsBox
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.minimumHeight: 130
                visible: root.currentView !== "Settings" && root.currentView !== "About"
                color: root.surface
                radius: 10
                border.color: results.activeFocus ? root.accent : root.line
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 1
                    spacing: 0
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.margins: 14
                        Controls.Label {
                            text: backend.writing && backend.status.length ? backend.status : root.viewItems.length + (root.currentView === "Sources" ? (root.viewItems.length === 1 ? " source" : " sources") : (root.viewItems.length === 1 ? " package" : " packages")) + (root.currentView === "Updates" ? " · " + root.selectedCount() + " selected" : "")
                            color: root.muted
                            font.pixelSize: 12
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
                        Controls.Label {
                            text: "Working…"
                            visible: backend.busy && !root.motionEnabled
                            color: root.muted
                            font.pixelSize: 12
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
                    Rectangle { Layout.fillWidth: true; height: 1; color: root.line }
                    RowLayout {
                        visible: !root.compact
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
                            font.pixelSize: 11
                            font.underline: sortNameArea.activeFocus
                            elide: Text.ElideRight
                            Layout.preferredWidth: root.nameWidth
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
                            text: (root.currentView === "Sources" ? "STATUS" : "VERSION") + root.sortArrow(root.currentView === "Sources" ? "status" : "version")
                            color: root.muted
                            font.pixelSize: 11
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
                            text: (root.currentView === "Sources" ? "CAPABILITIES" : "SUMMARY") + root.sortArrow(root.currentView === "Sources" ? "capabilities" : "summary")
                            color: root.muted
                            font.pixelSize: 11
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
                        onCountChanged: currentIndex = -1
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
                            width: ListView.view.width
                            height: (root.compact ? 78 : 56) + (modelData.groupStart ? 38 : 0)
                            topPadding: modelData.groupStart ? 38 : 0
                            leftPadding: 16
                            rightPadding: 16
                            highlighted: results.currentIndex === index
                            enabled: !backend.writing
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
                                        font.pixelSize: 11
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
                                    enabled: !backend.writing
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
                                    Layout.fillWidth: root.compact
                                    Controls.Label {
                                        text: modelData.display_name || modelData.name
                                        color: root.ink
                                        font.bold: true
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                    RowLayout {
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
                                        text: modelData.source.toUpperCase() + (modelData.remote ? " · " + modelData.remote : "") + (modelData.source === "flatpak" ? " · " + (modelData.scope === "system" ? "System" : "User") : "")
                                        color: root.muted
                                        font.pixelSize: 11
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                        }
                                    }
                                    Controls.Label {
                                        visible: root.compact
                                        text: modelData.kind === "source" ? (modelData.capabilities || []).join(", ") : (modelData.summary || "")
                                        color: root.muted
                                        textFormat: Text.PlainText
                                        elide: Text.ElideRight
                                        Layout.fillWidth: true
                                    }
                                }
                                Controls.Label {
                                    Layout.preferredWidth: root.compact ? 100 : root.versionWidth
                                    text: root.versionText(modelData)
                                    font.family: "monospace"
                                    color: modelData.kind === "failure" ? "#e87979" : (modelData.update === "available" ? root.accent : root.muted)
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                    font.pixelSize: 12
                                }
                                Controls.Label {
                                    visible: !root.compact
                                    Layout.fillWidth: true
                                    text: modelData.kind === "source" ? (modelData.capabilities || []).join(", ") : (modelData.summary || "")
                                    color: root.muted
                                    textFormat: Text.PlainText
                                    elide: Text.ElideRight
                                }
                                ActionButton {
                                    objectName: "rowPackageAction"
                                    visible: modelData.kind === "package" && (!root.updateOnly(modelData.source) || modelData.update === "available")
                                    enabled: !backend.busy && !root.retainingResults
                                    text: ""
                                    symbol: (root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "updates" : (root.isInstalled(modelData) ? "remove" : "install")
                                    glyphColor: (root.currentView === "Updates" || root.updateOnly(modelData.source)) ? root.accent : root.isInstalled(modelData) ? (root.dark ? "#f18b91" : "#b42332") : (root.dark ? "#77d6a0" : "#187442")
                                    Accessible.name: ((root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "Update " : (root.isInstalled(modelData) ? "Remove " : "Install ")) + (modelData.display_name || modelData.name) + " from " + modelData.source
                                    Layout.preferredWidth: 38
                                    horizontalPadding: 8
                                    onClicked: backend.propose((root.currentView === "Updates" || root.updateOnly(modelData.source)) ? "upgrade" : (root.isInstalled(modelData) ? "remove" : "install"), root.originalIndex(index))
                                }
                            }
                        }
                        Column {
                            anchors.centerIn: parent
                            width: parent.width - 32
                            spacing: 8
                            visible: results.count === 0
                            Controls.BusyIndicator {
                                running: backend.busy && root.motionEnabled
                                visible: running
                                anchors.horizontalCenter: parent.horizontalCenter
                                width: 32
                                height: 32
                            }
                            Controls.Label {
                                objectName: "emptyState"
                                width: parent.width
                                horizontalAlignment: Text.AlignHCenter
                                wrapMode: Text.WordWrap
                                color: root.muted
                                visible: !backend.busy || !root.motionEnabled
                                text: backend.busy ? "Working…" : root.currentView === "Search" ? (search.text.trim().length === 0 ? "Search apps and packages" : "No matching packages.\nTry a shorter search or another source.") : (root.currentView === "Updates" ? "You're up to date" : (root.currentView === "Installed" && (root.installedFilter.length > 0 || root.multiSourceOnly) ? "No packages match these filters.\nClear the filter or include more sources." : "No results to show.\nCheck source availability or reload to try again."))
                            }
                        }
                    }
                }
            }
            Rectangle {
                id: detailsPanel
                objectName: "detailsPanel"
                visible: root.selected !== null && ["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                Layout.preferredHeight: root.visibleScreenshots.length > 0 ? Math.min(root.height * (root.compact ? 0.35 : 0.42), 320) : Math.min(root.height * 0.27, 180)
                color: root.surface
                radius: 10
                border.color: root.line
                ColumnLayout {
                    id: detailsContent
                    objectName: "detailsContent"
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 6
                    RowLayout {
                        Layout.fillWidth: true
                        Item {
                            visible: detailIcon.status !== Image.Ready
                            Layout.preferredWidth: 40
                            Layout.preferredHeight: 40
                            DeckIcon {
                                name: root.selected ? (root.selected.kind === "source" ? root.selected.source : (root.selected.kind === "failure" ? "warning" : "package")) : "package"
                                ink: root.accent
                                anchors.centerIn: parent
                                width: 24
                                height: 24
                            }
                        }
                        Image {
                            id: detailIcon
                            visible: status === Image.Ready
                            asynchronous: true
                            source: root.iconUrl(root.selectedIcon)
                            sourceSize.width: 40
                            sourceSize.height: 40
                            fillMode: Image.PreserveAspectFit
                            Layout.preferredWidth: 40
                            Layout.preferredHeight: 40
                            Accessible.ignored: true
                        }
                        Controls.Label {
                        text: root.selected ? (root.selected.display_name || root.selected.name) : ""
                        textFormat: Text.PlainText
                        color: root.ink
                        font.pixelSize: root.compact ? 18 : 22
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                        }
                        ActionButton {
                            objectName: "closeDetailsButton"
                            text: ""
                            symbol: "cancel"
                            Accessible.name: "Close details"
                            Layout.preferredWidth: 38
                            horizontalPadding: 8
                            onClicked: {
                                results.currentIndex = -1;
                                root.selectedIdentity = null;
                            }
                        }
                    }
                    Controls.ScrollView {
                        id: detailScroll
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        contentWidth: availableWidth
                        clip: true
                        ColumnLayout {
                            width: detailScroll.availableWidth
                            spacing: 10
                            ListView {
                                id: screenshotGallery
                                objectName: "screenshotGallery"
                                visible: root.visibleScreenshots.length > 0
                                Layout.fillWidth: true
                                Layout.preferredHeight: root.compact ? 72 : 130
                                orientation: ListView.Horizontal
                                spacing: 10
                                clip: true
                                model: root.visibleScreenshots
                                cacheBuffer: 0
                                reuseItems: true
                                delegate: Controls.AbstractButton {
                                    required property var modelData
                                    width: root.compact ? 128 : 230
                                    height: screenshotGallery.height
                                    enabled: preview.status === Image.Ready
                                    Accessible.name: modelData.caption || "View app screenshot"
                                    onClicked: {
                                        root.screenshotUrl = modelData.url;
                                        root.screenshotCaption = modelData.caption || "";
                                        screenshotDialog.open();
                                    }
                                    background: Rectangle { color: root.canvas; radius: 6; border.color: parent.activeFocus ? root.accent : root.line }
                                    contentItem: Item {
                                        Image {
                                            id: preview
                                            onStatusChanged: {
                                                if (status === Image.Error) {
                                                    const failedUrl = modelData.url;
                                                    const identity = root.rowIdentity(root.selected);
                                                    Qt.callLater(root.hideFailedScreenshot, failedUrl, identity);
                                                }
                                            }
                                            anchors.fill: parent
                                            source: root.detailMatchesSelection ? modelData.url : ""
                                            asynchronous: true
                                            cache: true
                                            sourceSize.width: 960
                                            sourceSize.height: 540
                                            fillMode: Image.PreserveAspectFit
                                        }
                                        Controls.BusyIndicator {
                                            anchors.centerIn: parent
                                            width: 28; height: 28
                                            visible: preview.status === Image.Loading && root.motionEnabled
                                            running: visible
                                        }
                                        Controls.Label {
                                            anchors.centerIn: parent
                                            width: parent.width - 12
                                            horizontalAlignment: Text.AlignHCenter
                                            color: root.muted
                                            wrapMode: Text.WordWrap
                                            text: "Loading…"
                                            visible: preview.status === Image.Loading && !root.motionEnabled
                                        }
                                    }
                                }
                            }
                            TextEdit {
                                objectName: "packageDetails"
                                Layout.fillWidth: true
                                readOnly: true
                                selectByMouse: true
                                color: root.muted
                                padding: 0
                                font.pixelSize: 14
                                wrapMode: TextEdit.Wrap
                                textFormat: TextEdit.PlainText
                                text: root.detailMatchesSelection ? [root.detail.description || "",
                                    root.detail.package.reference || root.detail.package.name,
                                    [root.detail.package.scope_label, root.detail.package.architecture].filter(Boolean).join(" · "),
                                    root.detail.homepage || "",
                                    (root.detail.dependencies || []).length ? "Dependencies: " + root.detail.dependencies.join(", ") : ""].filter(Boolean).join("\n\n") : root.detail.failure ? ((root.detail.failure.error || "") + "\n" + (root.detail.hint || "")) : (root.detail.availability || "")
                                Accessible.name: "Package details"
                            }
                        }
                    }
                }
            }
            Flow {
                objectName: "updatesActions"
                Layout.fillWidth: true
                spacing: 8
                visible: ["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0
                ActionButton {
                    objectName: "upgradeAllButton"
                    visible: root.currentView === "Updates" && (root.uncheckedPackages.length === 0 || root.selectedCount() > 0)
                    text: root.uncheckedPackages.length === 0 ? "Update all" : "Update selected"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy && (root.uncheckedPackages.length > 0 || backend.upgradable)
                    onClicked: root.upgradeUpdates()
                }
                ActionButton {
                    objectName: "selectNoneButton"
                    visible: root.currentView === "Updates" && root.selectedCount() > 0
                    text: "Select none"
                    symbol: "cancel"
                    enabled: !backend.writing
                    onClicked: root.selectNonePackages()
                }
                ActionButton {
                    objectName: "selectAllButton"
                    visible: root.currentView === "Updates" && root.uncheckedPackages.length > 0
                    text: "Select all"
                    symbol: "installed"
                    enabled: !backend.writing
                    onClicked: root.uncheckedPackages = []
                }
                Controls.Label {
                    objectName: "upgradeAllHint"
                    visible: root.currentView === "Updates" && !backend.upgradable && !backend.busy && root.items.some((row) => row.kind === "failure")
                    text: "Update all is unavailable while a source has failed."
                    color: root.muted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "repositoriesButton"
                    visible: root.currentView === "Sources"
                    text: "Repositories"
                    symbol: "sources"
                    enabled: !backend.busy
                    onClicked: repositoriesDialog.open()
                }
                ActionButton {
                    objectName: "refreshButton"
                    visible: root.selected !== null && root.selected.kind === "source"
                    text: "Refresh sources"
                    symbol: "refresh"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "source" && root.selected.available
                    onClicked: root.propose("refresh")
                }
                ActionButton {
                    text: "Reload"
                    symbol: "refresh"
                    enabled: !backend.writing
                    onClicked: root.reload(true)
                }
            }
        }
    }
    Controls.Dialog {
        id: repositoriesDialog
        objectName: "repositoriesDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 850)
        height: Math.min(root.height - 40, 640)
        title: "Repositories"
        modal: true
        standardButtons: Controls.Dialog.Close
        onOpened: backend.loadRepositories()
        contentItem: ColumnLayout {
            spacing: 12
            Flow {
                Layout.fillWidth: true
                spacing: 8
                ActionButton {
                    text: "Add Flatpak repository"; symbol: "install"
                    enabled: !backend.busy
                    onClicked: addRepositoryDialog.open()
                }
                ActionButton {
                    text: "Software Sources"; symbol: "settings"
                    enabled: !backend.busy
                    onClicked: root.repositoryChange({backend: "apt", name: "sources", scope: "system"}, "open_editor")
                }
                ActionButton { text: ""; symbol: "refresh"; Accessible.name: "Reload repositories"; enabled: !backend.busy; onClicked: backend.loadRepositories() }
            }
            Controls.BusyIndicator { visible: backend.busy; running: visible; Layout.alignment: Qt.AlignHCenter }
            ListView {
                objectName: "repositoryList"
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                spacing: 8
                model: root.repositoryReport.repositories || []
                Controls.ScrollBar.vertical: Controls.ScrollBar {}
                delegate: Rectangle {
                    required property var modelData
                    width: ListView.view.width
                    height: 76
                    color: root.surface
                    radius: 6
                    RowLayout {
                        anchors.fill: parent; anchors.margins: 8; spacing: 10
                        Controls.CheckBox {
                            objectName: "repositoryEnabled"
                            checked: modelData.enabled
                            enabled: !backend.busy && modelData.backend !== "apt"
                            Accessible.name: "Enable " + (modelData.title || modelData.name)
                            onClicked: {
                                root.repositoryChange(modelData, "set_enabled", {enabled: checked});
                                checked = Qt.binding(() => modelData.enabled);
                            }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 3
                            Controls.Label { text: modelData.title || modelData.name; textFormat: Text.PlainText; color: root.ink; elide: Text.ElideRight; Layout.fillWidth: true }
                            Controls.Label { text: modelData.backend.toUpperCase() + " · " + (modelData.scope === "system" ? "System" : "User") + (modelData.url ? " · " + modelData.url : ""); textFormat: Text.PlainText; color: root.muted; elide: Text.ElideRight; Layout.fillWidth: true; font.pixelSize: 11 }
                        }
                        Controls.Label { visible: modelData.priority !== null; text: "Priority " + modelData.priority; color: root.muted; font.pixelSize: 11 }
                        ActionButton {
                            text: ""; symbol: "up"; visible: modelData.backend === "flatpak"
                            Accessible.name: "Increase repository priority"; enabled: !backend.busy && modelData.priority < 9999
                            onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority + 1})
                        }
                        ActionButton {
                            text: ""; symbol: "down"; visible: modelData.backend === "flatpak"
                            Accessible.name: "Decrease repository priority"; enabled: !backend.busy && modelData.priority > 0
                            onClicked: root.repositoryChange(modelData, "set_priority", {priority: modelData.priority - 1})
                        }
                        ActionButton {
                            text: ""; symbol: "settings"; visible: modelData.backend === "apt"
                            Accessible.name: "Edit software sources"; enabled: !backend.busy
                            onClicked: root.repositoryChange({backend: "apt", name: "sources", scope: "system"}, "open_editor")
                        }
                        ActionButton {
                            objectName: "removeRepositoryButton"
                            text: ""; symbol: "remove"; visible: modelData.backend === "flatpak"
                            Accessible.name: "Remove " + modelData.name; enabled: !backend.busy
                            onClicked: root.repositoryChange(modelData, "remove")
                        }
                    }
                }
            }
            Controls.Label { Layout.fillWidth: true; color: root.muted; textFormat: Text.PlainText; wrapMode: Text.WordWrap; text: (root.repositoryReport.errors || []).join("\n"); visible: text.length > 0 }
            Controls.Label { Layout.fillWidth: true; color: root.muted; textFormat: Text.PlainText; wrapMode: Text.WordWrap; text: backend.status }
        }
    }
    Controls.Dialog {
        id: addRepositoryDialog
        objectName: "addRepositoryDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 48, 560)
        title: "Add Flatpak repository"
        modal: true
        standardButtons: Controls.Dialog.Ok | Controls.Dialog.Cancel
        onOpened: { repositoryName.text = ""; repositoryUrl.text = ""; repositoryName.forceActiveFocus(); }
        onAccepted: root.repositoryChange({backend: "flatpak", name: repositoryName.text.trim(), scope: repositoryScope.currentIndex === 0 ? "user" : "system"}, "add", {url: repositoryUrl.text.trim()})
        contentItem: ColumnLayout {
            Controls.TextField { id: repositoryName; objectName: "repositoryName"; placeholderText: "Name"; Accessible.name: "Repository name"; Layout.fillWidth: true }
            Controls.TextField { id: repositoryUrl; objectName: "repositoryUrl"; placeholderText: "https://…/repository.flatpakrepo"; Accessible.name: "Repository URL"; Layout.fillWidth: true }
            Controls.ComboBox { id: repositoryScope; objectName: "repositoryScope"; model: ["User", "System"]; Accessible.name: "Installation scope"; Layout.fillWidth: true }
        }
    }
    Controls.Dialog {
        id: screenshotDialog
        objectName: "screenshotDialog"
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 1040)
        height: Math.min(root.height - 32, 720)
        modal: true
        title: root.screenshotCaption || "Screenshot"
        standardButtons: Controls.Dialog.Close
        onClosed: root.screenshotUrl = ""
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
    Controls.Dialog {
        id: confirmation
        objectName: "confirmationDialog"
        parent: Controls.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - 32, 600)
        height: Math.min(root.height - 32, 340)
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
        standardButtons: Controls.Dialog.Yes | Controls.Dialog.No
        onOpened: {
            // Let the buttons own their mnemonics. Separate Shortcuts collide
            // with the automatic button mnemonics in KDE styles.
            standardButton(Controls.Dialog.Yes).text = "&Yes";
            standardButton(Controls.Dialog.No).text = "&No";
            standardButton(Controls.Dialog.No).forceActiveFocus();
        }
        onAccepted: backend.confirm(true)
        onRejected: backend.confirm(false)
        contentItem: Controls.ScrollView {
            id: confirmationScroll
            contentWidth: availableWidth
            clip: true
            TextEdit {
                width: confirmationScroll.availableWidth
                color: root.ink
                font.pixelSize: 14
                text: backend.confirmation
                readOnly: true
                selectByMouse: true
                wrapMode: TextEdit.Wrap
                textFormat: TextEdit.PlainText
            }
        }
    }
    Shortcut {
        sequence: "Ctrl+Q"
        onActivated: root.close()
    }
    Shortcut {
        sequence: "Ctrl+L"
        enabled: !backend.writing
        onActivated: results.forceActiveFocus()
    }
    Shortcut {
        sequence: "Ctrl+F"
        enabled: !backend.writing
        onActivated: {
            root.openView("Search");
            search.selectAll();
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
        onActivated: root.openView("Sources")
    }
    Shortcut {
        sequence: "Ctrl+R"
        enabled: !backend.writing
        onActivated: root.reload(true)
    }
    Shortcut {
        sequence: "Ctrl+I"
        enabled: !backend.busy
        onActivated: root.propose("install")
    }
    Shortcut {
        sequence: "Ctrl+D"
        enabled: !backend.busy
        onActivated: root.propose("remove")
    }
    Shortcut {
        sequence: "Ctrl+U"
        enabled: !backend.busy
        onActivated: root.propose("upgrade")
    }
    Shortcut {
        sequence: "Ctrl+Shift+U"
        enabled: root.currentView === "Updates" && !backend.busy && root.selectedCount() > 0 && (root.uncheckedPackages.length > 0 || backend.upgradable)
        onActivated: root.upgradeUpdates()
    }
    Shortcut {
        sequence: "Ctrl+M"
        enabled: !backend.busy
        onActivated: root.propose("refresh")
    }
    Shortcut {
        sequence: "Escape"
        enabled: backend.busy
        onActivated: backend.cancel()
    }
}
