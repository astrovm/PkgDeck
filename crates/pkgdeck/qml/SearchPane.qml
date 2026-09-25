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
        leftPadding: 42
        rightPadding: 42
        background: Rectangle {
            color: pane.surface
            radius: 10
            border.color: search.activeFocus ? pane.accent : pane.line
            border.width: search.activeFocus ? 2 : 1
            Behavior on border.color { ColorAnimation { duration: 120 } }
            DeckIcon {
                name: "search"
                ink: search.activeFocus ? pane.accent : pane.muted
                anchors.left: parent.left
                anchors.leftMargin: 14
                anchors.verticalCenter: parent.verticalCenter
                width: 18
                height: 18
            }
        }
        onAccepted: pane.submitted()
        Keys.onDownPressed: pane.downRequested()
        onTextChanged: pane.queryEdited()
        ClearFieldButton {
            objectName: "clearSearchButton"
            anchors.right: parent.right
            anchors.rightMargin: 5
            anchors.verticalCenter: parent.verticalCenter
            visible: search.text.length > 0
            enabled: search.enabled
            ink: pane.muted
            hoverColor: pane.line
            clearLabel: "Clear search"
            onClicked: { search.clear(); search.forceActiveFocus(); }
        }
    }
}
