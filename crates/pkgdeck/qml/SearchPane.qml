import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

RowLayout {
    id: pane
    objectName: "searchPane"
    property alias text: search.text
    property bool writing: false
    property color surface
    property color ink
    property color muted
    property color line
    property color accent
    property color onAccent
    property font textFont
    signal submitted()
    signal downRequested()
    signal queryEdited()

    spacing: 8
    Layout.fillWidth: true

    function focusSearch(selectAll) {
        search.forceActiveFocus();
        if (selectAll)
            search.selectAll();
    }
    Controls.TextField {
        id: search
        objectName: "searchField"
        Layout.fillWidth: true
        placeholderText: "Search apps and packages"
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
        objectName: "searchButton"
        text: "Search"
        Accessible.name: text
        enabled: !pane.writing && search.text.trim().length > 0
        implicitWidth: Math.max(104, buttonContents.implicitWidth + leftPadding + rightPadding)
        Layout.minimumWidth: implicitWidth
        implicitHeight: search.implicitHeight
        horizontalPadding: 14
        onClicked: pane.submitted()
        background: Rectangle {
            radius: 7
            color: parent.enabled ? pane.accent : pane.surface
            border.color: parent.activeFocus ? pane.accent : pane.line
            border.width: parent.activeFocus ? 2 : 1
        }
        contentItem: RowLayout {
            id: buttonContents
            spacing: 8
            DeckIcon { name: "search"; ink: searchButton.enabled ? pane.onAccent : pane.muted; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
            Text { objectName: "searchButtonLabel"; text: "Search"; color: searchButton.enabled ? pane.onAccent : pane.muted; font: pane.textFont }
        }
    }
}
