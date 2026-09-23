import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane
    objectName: "searchPane"
    property alias text: search.text
    property string contentFilter: "All"
    property bool writing: false
    property bool compact: false
    property color surface
    property color ink
    property color muted
    property color line
    property color accent
    property color onAccent
    property color selection
    property font textFont
    signal submitted()
    signal downRequested()
    signal queryEdited()

    readonly property var filters: ["All", "Apps", "CLI tools", "System packages", "Container images"]
    spacing: 8
    Layout.fillWidth: true

    function focusSearch(selectAll) {
        search.forceActiveFocus();
        if (selectAll)
            search.selectAll();
    }
    // A source alone only proves its type when that manager has one clear
    // purpose. Native catalogs are mixed; AppStream ids identify their apps.
    function category(row) {
        if (!row || row.kind !== "package")
            return "";
        if (["docker", "podman"].indexOf(row.source) >= 0)
            return "Container images";
        if ((row.component_ids || []).length > 0 || ["flatpak", "appimage", "homebrew-cask"].indexOf(row.source) >= 0)
            return "Apps";
        if (["cargo", "npm", "pnpm", "bun", "pip", "pipx", "uv", "composer", "gem", "codex", "claude", "grok", "opencode"].indexOf(row.source) >= 0)
            return "CLI tools";
        if (["apt", "dnf", "pacman", "zypper", "fwupd"].indexOf(row.source) >= 0)
            return "System packages";
        return "";
    }
    function accepts(row) {
        return contentFilter === "All" || category(row) === contentFilter;
    }
    function filterAt(name) {
        const index = filters.indexOf(name);
        return index < 0 ? null : chips.itemAt(index);
    }

    RowLayout {
        Layout.fillWidth: true
        Controls.TextField {
            id: search
            objectName: "searchField"
            Layout.fillWidth: true
            placeholderText: pane.contentFilter === "Container images" ? "Search local images or enter an exact image reference" : "Search apps and packages"
            Accessible.name: "Search packages"
            enabled: !pane.writing
            selectByMouse: true
            implicitHeight: Math.max(44, pane.textFont.pointSize * 3.4)
            color: pane.ink
            placeholderTextColor: pane.muted
            leftPadding: 14
            background: Rectangle {
                color: pane.surface
                radius: 8
                border.color: search.activeFocus ? pane.accent : pane.line
                border.width: search.activeFocus ? 2 : 1
            }
            onAccepted: pane.submitted()
            Keys.onDownPressed: pane.downRequested()
            onTextChanged: pane.queryEdited()
        }
        Controls.Button {
            id: searchButton
            text: "Search"
            Accessible.name: text
            enabled: !pane.writing && search.text.trim().length > 0
            implicitHeight: Math.max(38, pane.textFont.pointSize * 3)
            onClicked: pane.submitted()
            background: Rectangle {
                radius: 7
                color: parent.enabled ? pane.accent : pane.surface
                border.color: parent.activeFocus ? pane.accent : pane.line
                border.width: parent.activeFocus ? 2 : 1
            }
            contentItem: RowLayout {
                spacing: 8
                DeckIcon { name: "search"; ink: searchButton.enabled ? pane.onAccent : pane.muted; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
                Text { text: "Search"; color: searchButton.enabled ? pane.onAccent : pane.muted; font: pane.textFont }
            }
        }
    }
    Flickable {
        Layout.fillWidth: true
        Layout.preferredHeight: 30
        contentWidth: chipRow.width
        contentHeight: height
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        Row {
            id: chipRow
            spacing: 6
            Repeater {
                id: chips
                model: pane.filters
                delegate: Controls.Button {
                    required property string modelData
                    objectName: "contentFilter-" + modelData
                    text: modelData
                    checkable: true
                    checked: pane.contentFilter === modelData
                    implicitHeight: 30
                    Accessible.name: "Show " + modelData
                    onClicked: pane.contentFilter = modelData
                    background: Rectangle {
                        radius: 6
                        color: parent.checked ? pane.selection : pane.surface
                        border.color: parent.activeFocus || parent.checked ? pane.accent : pane.line
                    }
                    contentItem: Text {
                        text: modelData
                        color: pane.ink
                        font: pane.textFont
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                        leftPadding: 8
                        rightPadding: 8
                    }
                }
            }
        }
    }
}
