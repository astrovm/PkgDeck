import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import QtCore
import org.kde.kirigami as Kirigami

Controls.ApplicationWindow {
    id: root
    required property var backend
    property string currentView: "Search"
    property string resultView: "Search"
    property var items: currentView === resultView ? JSON.parse(backend.rows || "[]") : []
    property var detail: JSON.parse(backend.details || "{}")
    property bool closePending: false
    property bool queryDirty: false
    property var selectedIdentity: null
    function rowIdentity(row) {
        return row ? JSON.stringify([row.source, row.name, row.architecture, row.remote || null, row.scope]) : "";
    }
    property var selected: results.currentIndex >= 0 && results.currentIndex < viewItems.length ? viewItems[results.currentIndex] : null
    // Explicitly checked source ids, comma-joined; empty means every
    // available source. Unchecking hides a source from all queries, which
    // is how backends you never use stay silent.
    property string sourceSelection: ""
    property string installedFilter: ""
    property bool useSudo: argument("--auth", preferences.authorization) === "sudo"
    readonly property var sourceIds: ["apt", "dnf", "pacman", "zypper", "snap", "homebrew", "appimage", "flatpak", "cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem"]
    readonly property var sourceNames: ["APT", "DNF", "Pacman", "Zypper", "Snap", "Homebrew", "AppImage", "Flatpak", "Cargo", "npm", "pnpm", "Bun", "pip", "pipx", "uv", "Composer", "RubyGems"]
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
            return "All available sources";
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
    // Checked package identities for the Updates multi-select. Identities,
    // not indexes: streaming partials re-sort rows, so the controller
    // re-resolves each identity and skips stale ones, never guessing.
    property var checkedPackages: []
    function togglePackage(row) {
        const id = rowIdentity(row);
        if (!id)
            return;
        const checked = checkedPackages.slice();
        const at = checked.indexOf(id);
        if (at >= 0)
            checked.splice(at, 1);
        else
            checked.push(id);
        checkedPackages = checked;
    }
    function selectAllPackages() {
        const all = [];
        for (let i = 0; i < root.viewItems.length; i++) {
            if (root.viewItems[i].kind === "package") {
                const id = rowIdentity(root.viewItems[i]);
                if (id && all.indexOf(id) < 0)
                    all.push(id);
            }
        }
        checkedPackages = all;
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
    property var viewItems: {
        const rows = items.slice();
        if (sortColumn !== "") {
            const column = sortColumn;
            const dir = sortAscending ? 1 : -1;
            rows.sort((a, b) => {
                const x = sortValue(column, a).toLowerCase();
                const y = sortValue(column, b).toLowerCase();
                return x < y ? -dir : (x > y ? dir : 0);
            });
        }
        return rows;
    }
    // The visible index addresses viewItems; the backend addresses items.
    // With no active sort the two orders match and this is the identity.
    // Sorted duplicates resolve to the first raw match: identical rows are
    // one engine identity shown twice, so either index acts on the same one.
    function originalIndex(visible) {
        if (visible < 0 || visible >= viewItems.length)
            return -1;
        if (sortColumn === "")
            return visible;
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
        property bool primary: false
        property bool navigation: false
        property string symbol: "package"
        Accessible.name: text
        implicitHeight: 38
        horizontalPadding: 16
        opacity: enabled ? 1 : 0.45
        background: Rectangle {
            radius: 7
            color: control.primary && control.enabled ? root.accent : (control.hovered ? root.selection : root.surface)
            border.color: control.activeFocus ? root.accent : (control.navigation ? "transparent" : root.line)
            border.width: control.activeFocus ? 2 : 1
        }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon {
                name: control.symbol
                ink: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : root.ink
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
            }
            Text {
                text: control.text
                font: control.font
                color: control.primary && control.enabled ? (root.dark ? "#111820" : "#ffffff") : root.ink
                Layout.fillWidth: control.navigation
                verticalAlignment: Text.AlignVCenter
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

    function rowTooltip(data) {
        return data.name + "\n" + (data.installed || "not installed") + " → " + (data.candidate || "unknown") + "\n" + (data.summary || "");
    }
    // Local icon files become file:// URLs. Paths come from the backend and
    // may contain spaces, which raw concatenation would leave unencoded and
    // unloadable (hiding the fallback source icon with a blank gap).
    function iconUrl(path) {
        return path ? "file://" + encodeURI(path) : "";
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
        if (backend.busy)
            return;
        queryDirty = false;
        selectedIdentity = null;
        checkedPackages = [];
        currentView = view;
        results.currentIndex = -1;
        if (view === "Search")
            search.forceActiveFocus();
        else if (["Search", "Installed", "Updates", "Sources"].indexOf(view) >= 0)
            reload();
    }
    function reload() {
        resultView = currentView;
        results.currentIndex = -1;
        selectedIdentity = null;
        checkedPackages = [];
        backend.load(currentView, currentView === "Installed" ? installedFilter : search.text, checkedCsv(), useSudo);
    }
    function choose(index) {
        if (index < 0 || index >= viewItems.length)
            return;
        results.currentIndex = index;
        selectedIdentity = rowIdentity(viewItems[index]);
        backend.select(originalIndex(index));
    }
    function propose(action) {
        backend.propose(action, originalIndex(results.currentIndex));
    }
    Settings {
        id: preferences
        category: "Browser"
        property int appearance: 0
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
        function onBusyChanged() {
            if (!backend.busy && root.closePending)
                root.close();
            else if (!backend.busy && root.selected !== null)
                results.forceActiveFocus();
        }
        function onRowsChanged() {
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
            resultsBox.opacity = 0.35;
            resultsBox.opacity = 1;
            if (root.queryDirty) {
                root.queryDirty = false;
            } else if (root.viewItems.length > 0) {
                results.forceActiveFocus();
            }
        }
        function onConfirmationChanged() {
            if (backend.confirmation.length)
                confirmation.open();
            else
                confirmation.close();
        }
    }
    Timer {
        interval: 40
        running: backend.busy
        repeat: true
        onTriggered: backend.poll()
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
                        enabled: !backend.busy
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
                        Controls.ToolTip.visible: hovered
                        Controls.ToolTip.text: root.repositoryUrl
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
                    enabled: !backend.busy
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
                    text: root.sourceSummary()
                    symbol: "sources"
                    enabled: !backend.busy
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
                    placeholderText: "Search packages"
                    Accessible.name: "Search packages"
                    enabled: !backend.busy
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
                        root.reload();
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
                    enabled: !backend.busy && search.text.trim().length > 0
                    onClicked: { root.currentView = "Search"; root.queryDirty = false; root.reload(); }
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
                    enabled: !backend.busy
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
                    onAccepted: {
                        root.installedFilter = text;
                        root.reload();
                    }
                }
                ActionButton {
                    text: "Filter"
                    symbol: "search"
                    primary: true
                    enabled: !backend.busy
                    onClicked: { root.installedFilter = installedFilterField.text; root.reload(); }
                }
            }
            ColumnLayout {
                visible: root.currentView === "Settings"
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
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
                Controls.Label {
                    text: "Package sources"
                }
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    color: root.muted
                    text: "Choose which sources every section queries using the source selector in the header."
                }
                Controls.Label {
                    text: "Privilege elevation"
                }
                ThemedComboBox {
                    objectName: "authorizationSetting"
                    model: ["Host polkit agent", "Existing sudo credentials"]
                    currentIndex: root.useSudo ? 1 : 0
                    onActivated: {
                        root.useSudo = currentIndex === 1;
                        preferences.authorization = root.useSudo ? "sudo" : "polkit";
                    }
                    Accessible.name: "Privilege elevation"
                    Layout.fillWidth: true
                    Layout.maximumWidth: 420
                }
                Controls.Label {
                    Layout.fillWidth: true
                    wrapMode: Text.WordWrap
                    text: "Polkit uses your host's authentication agent. Sudo requires an existing grant; passwords are never collected here. Homebrew and development managers always run unprivileged without elevation. Choose the appearance above, or follow your system theme."
                }
            }
            Controls.Label {
                objectName: "aboutText"
                visible: root.currentView === "About"
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: "PkgDeck " + backend.version + "\nA unified package interface for Linux.\n\nKeyboard shortcuts\nCtrl+1: Search • Ctrl+2: Installed • Ctrl+3: Updates • Ctrl+4: Sources\nCtrl+F: search • Ctrl+L: focus results • Up/Down: select • Ctrl+I: install • Ctrl+D: remove • Ctrl+U: upgrade • Ctrl+M: refresh source • Ctrl+R: reload\nEscape: cancel current work\n\nRefresh updates source metadata; Upgrade changes an installed package. Writes require confirmation and may change native dependencies. Cancellation waits for a native write already running.\n\nSearches show configured package sources. Check the status area for a source that is unavailable or did not respond."
                textFormat: Text.PlainText
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
                opacity: 1
                Behavior on opacity {
                    NumberAnimation { duration: 120 }
                }
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 1
                    spacing: 0
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.margins: 14
                        Controls.Label {
                            text: root.viewItems.length + (root.currentView === "Sources" ? (root.viewItems.length === 1 ? " source" : " sources") : (root.viewItems.length === 1 ? " package" : " packages")) + (root.currentView === "Updates" && root.checkedPackages.length > 0 ? " · " + root.checkedPackages.length + " selected" : "")
                            color: root.muted
                            font.pixelSize: 12
                            Layout.fillWidth: true
                        }
                        Controls.Label { text: "↑ ↓ Select"; color: root.muted; font.pixelSize: 12 }
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
                            width: ListView.view.width
                            height: root.compact ? 78 : 56
                            leftPadding: 16
                            rightPadding: 16
                            highlighted: results.currentIndex === index
                            enabled: !backend.busy
                            Accessible.name: (modelData.kind === "package" ? (modelData.update === "available" ? "Update available. " : (modelData.installed ? "Installed. " : "Not installed. ")) : "") + modelData.name + ", " + modelData.source + ", " + (modelData.summary || "")
                            onClicked: { results.forceActiveFocus(); root.choose(index); }
                            onHoveredChanged: {
                                if (hovered && modelData.kind === "package")
                                    tipDelay.restart();
                                else {
                                    tipDelay.stop();
                                    tipCard.visible = false;
                                }
                            }
                            Timer {
                                id: tipDelay
                                interval: 400
                                onTriggered: tipCard.visible = true
                            }
                            Rectangle {
                                id: tipCard
                                visible: false
                                z: 10
                                anchors.left: parent.left
                                anchors.right: parent.right
                                anchors.leftMargin: 16
                                anchors.rightMargin: 16
                                y: packageRow.height + 2
                                height: tipText.implicitHeight + 16
                                color: root.surface
                                border.color: root.line
                                radius: 6
                                Text {
                                    id: tipText
                                    anchors.fill: parent
                                    anchors.margins: 8
                                    text: root.rowTooltip(modelData)
                                    color: root.ink
                                    wrapMode: Text.WordWrap
                                    textFormat: Text.PlainText
                                }
                            }
                            background: Rectangle {
                                color: packageRow.highlighted ? root.selection : (packageRow.hovered ? root.canvas : "transparent")
                                Rectangle { width: 3; height: parent.height; visible: packageRow.highlighted; color: root.accent }
                                Rectangle { anchors.bottom: parent.bottom; width: parent.width; height: 1; color: root.line; opacity: 0.5 }
                            }
                            contentItem: RowLayout {
                                spacing: 14
                                Controls.CheckBox {
                                    id: packageCheck
                                    visible: root.currentView === "Updates" && modelData.kind === "package"
                                    checked: root.checkedPackages.indexOf(root.rowIdentity(modelData)) >= 0
                                    enabled: !backend.busy
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
                                        text: modelData.name
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
                                            visible: !modelData.icon
                                            name: modelData.source
                                            ink: root.muted
                                            Layout.preferredWidth: 14
                                            Layout.preferredHeight: 14
                                        }
                                        Image {
                                            visible: !!modelData.icon
                                            source: root.iconUrl(modelData.icon || "")
                                            sourceSize.width: 14
                                            sourceSize.height: 14
                                            fillMode: Image.PreserveAspectFit
                                            Layout.preferredWidth: 14
                                            Layout.preferredHeight: 14
                                            Accessible.ignored: true
                                        }
                                        Controls.Label {
                                        text: modelData.source.toUpperCase() + (modelData.remote ? " · " + modelData.remote : "") + (modelData.architecture ? " · " + modelData.architecture : "")
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
                                    text: modelData.kind === "failure" ? "Failed" : (modelData.kind === "source" ? (modelData.available ? "Available" : "Unavailable") : (modelData.installed ? modelData.installed + (modelData.update === "available" ? " → " + modelData.candidate : " · installed") : modelData.candidate || "Unknown"))
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
                            }
                        }
                        Controls.Label {
                            anchors.centerIn: parent
                            width: parent.width - 32
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                            color: root.muted
                            visible: results.count === 0
                            text: backend.busy ? "Loading packages…" : root.currentView === "Search" ? "Search by name or description.\nIf nothing matches, try a shorter search or another source." : "No results to show.\nCheck source availability or reload to try again."
                        }
                    }
                }
            }
            Rectangle {
                visible: root.selected !== null && ["Search", "Installed", "Updates", "Sources"].indexOf(root.currentView) >= 0
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(root.height * 0.27, 180)
                color: root.surface
                radius: 10
                border.color: root.line
                ColumnLayout {
                    anchors.fill: parent
                    anchors.margins: 16
                    spacing: 6
                    RowLayout {
                        Layout.fillWidth: true
                        Item {
                            visible: !(root.selected && root.selected.icon)
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
                            visible: !!(root.selected && root.selected.icon)
                            source: root.iconUrl((root.selected && root.selected.icon) || "")
                            sourceSize.width: 40
                            sourceSize.height: 40
                            fillMode: Image.PreserveAspectFit
                            Layout.preferredWidth: 40
                            Layout.preferredHeight: 40
                            Accessible.ignored: true
                        }
                        Controls.Label {
                        text: root.selected ? root.selected.name : ""
                        textFormat: Text.PlainText
                        color: root.ink
                        font.pixelSize: root.compact ? 18 : 22
                        font.bold: true
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                        }
                    }
                    Controls.ScrollView {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        Controls.TextArea {
                            objectName: "packageDetails"
                            readOnly: true
                            selectByMouse: true
                            color: root.muted
                            padding: 0
                            background: null
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            text: root.detail.package ? (root.detail.description || "") + "\n\nScope: " + (root.detail.package.scope_label || "Unknown") + "   ·   Homepage: " + (root.detail.homepage || "Unavailable") + "\nDependencies: " + ((root.detail.dependencies || []).join(", ") || "None listed") : (root.detail.failure ? (root.detail.failure.error || "") + "\n\n" + (root.detail.hint || "") : (root.detail.availability || "") + "\n\nCapabilities: " + (root.detail.capabilities || []).join(", "))
                            Accessible.name: "Selected package, source, or failure details"
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
                    visible: root.currentView === "Updates"
                    text: "Upgrade all"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy && backend.upgradable
                    onClicked: root.propose("upgrade-all")
                }
                ActionButton {
                    objectName: "selectAllButton"
                    visible: root.currentView === "Updates"
                    text: "Select all"
                    symbol: "installed"
                    enabled: !backend.busy
                    onClicked: root.selectAllPackages()
                }
                ActionButton {
                    objectName: "selectNoneButton"
                    visible: root.currentView === "Updates" && root.checkedPackages.length > 0
                    text: "Select none"
                    symbol: "cancel"
                    enabled: !backend.busy
                    onClicked: root.checkedPackages = []
                }
                ActionButton {
                    objectName: "upgradeSelectedButton"
                    visible: root.currentView === "Updates" && root.checkedPackages.length > 0
                    text: "Upgrade selected"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy
                    onClicked: backend.proposeChecked(JSON.stringify(root.checkedPackages))
                }
                Controls.Label {
                    objectName: "upgradeAllHint"
                    visible: root.currentView === "Updates" && !backend.upgradable && !backend.busy && root.items.some((row) => row.kind === "failure")
                    text: "Upgrade all is unavailable while a source query fails. Select a failed row for details."
                    color: root.muted
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                }
                ActionButton {
                    objectName: "installButton"
                    visible: root.selected !== null && root.selected.kind === "package" && !root.selected.installed
                    text: "Install"
                    symbol: "install"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "package" && !root.selected.installed
                    onClicked: root.propose("install")
                }
                ActionButton {
                    objectName: "removeButton"
                    visible: root.selected !== null && !!root.selected.installed
                    text: "Remove"
                    symbol: "remove"
                    enabled: !backend.busy && root.selected !== null && !!root.selected.installed
                    onClicked: root.propose("remove")
                }
                ActionButton {
                    objectName: "upgradeButton"
                    visible: root.selected !== null && root.selected.update === "available"
                    text: "Upgrade"
                    symbol: "updates"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.update === "available"
                    onClicked: root.propose("upgrade")
                }
                ActionButton {
                    objectName: "refreshButton"
                    visible: root.selected !== null && root.selected.kind === "source"
                    text: "Refresh source"
                    symbol: "refresh"
                    primary: true
                    enabled: !backend.busy && root.selected !== null && root.selected.kind === "source" && root.selected.available
                    onClicked: root.propose("refresh")
                }
                ActionButton {
                    text: "Reload"
                    symbol: "refresh"
                    enabled: !backend.busy
                    onClicked: root.reload()
                }
            }
            RowLayout {
                Layout.fillWidth: true
                DeckIcon {
                    name: "help"
                    ink: root.muted
                    visible: !backend.busy
                    Layout.alignment: Qt.AlignTop
                    Layout.preferredWidth: 18
                    Layout.preferredHeight: 18
                }
                Controls.BusyIndicator {
                    running: backend.busy
                    visible: running
                    Layout.preferredWidth: 32
                    Layout.preferredHeight: 32
                }
                Controls.ScrollView {
                    Layout.fillWidth: true
                    Layout.preferredHeight: root.compact ? 42 : 44
                    Controls.TextArea {
                        objectName: "operationStatus"
                        color: root.muted
                        background: null
                        font.pixelSize: 12
                        padding: 0
                        text: backend.status
                        readOnly: true
                        selectByMouse: true
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        Accessible.name: "Operation status"
                    }
                }
                ActionButton {
                    text: "Cancel"
                    symbol: "cancel"
                    visible: backend.busy
                    onClicked: backend.cancel()
                }
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
        title: "Confirm package operation"
        modal: true
        standardButtons: Controls.Dialog.Yes | Controls.Dialog.No
        onOpened: standardButton(Controls.Dialog.No).forceActiveFocus()
        Shortcut {
            sequence: "Alt+Y"
            enabled: confirmation.opened
            onActivated: confirmation.accept()
        }
        Shortcut {
            sequence: "Alt+N"
            enabled: confirmation.opened
            onActivated: confirmation.reject()
        }
        onAccepted: backend.confirm(true)
        onRejected: backend.confirm(false)
        contentItem: Controls.ScrollView {
            Controls.TextArea {
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
        enabled: !backend.busy
        onActivated: results.forceActiveFocus()
    }
    Shortcut {
        sequence: "Ctrl+F"
        enabled: !backend.busy
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
        enabled: !backend.busy
        onActivated: root.reload()
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
        enabled: root.currentView === "Updates" && !backend.busy && backend.upgradable
        onActivated: root.propose("upgrade-all")
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
