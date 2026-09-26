import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

RowLayout {
    id: pane
    objectName: "searchPane"
    property alias text: search.text
    property bool writing: false
    property color surface: Theme.surface
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color line: Theme.line
    property color accent: Theme.accent
    property color onAccent: Theme.accentInk
    property font textFont: Theme.baseFont
    // The 2 px ring is for keyboard focus; a click shows a quieter accent edge.
    readonly property bool keyboardFocus: search.activeFocus
        && [Qt.TabFocusReason, Qt.BacktabFocusReason, Qt.ShortcutFocusReason].indexOf(search.focusReason) >= 0
    signal submitted()
    signal downRequested()
    signal queryEdited()

    spacing: 8
    Layout.fillWidth: true

    // reason: pass Qt.ShortcutFocusReason when a key shortcut asks for the
    // field, so the keyboard focus ring shows.
    function focusSearch(selectAll, reason) {
        search.forceActiveFocus(reason === undefined ? Qt.OtherFocusReason : reason);
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
            radius: Theme.controlRadius
            border.color: search.activeFocus ? pane.accent : pane.line
            border.width: pane.keyboardFocus ? 2 : 1
            Behavior on border.color { ColorAnimation { duration: Theme.feedbackDuration } }
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
            readonly property bool shown: search.text.length > 0
            anchors.right: parent.right
            anchors.rightMargin: 5
            anchors.verticalCenter: parent.verticalCenter
            // Fades in with the first character. It leaves at once when
            // cleared, so it never lingers over an empty field.
            visible: shown
            opacity: shown ? 1 : 0
            scale: shown || !Theme.motionEnabled ? 1 : 0.8
            Behavior on opacity { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
            Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
            enabled: search.enabled && shown
            ink: pane.muted
            hoverColor: Theme.hoverTint
            clearLabel: "Clear search"
            onClicked: { search.clear(); search.forceActiveFocus(); }
        }
    }
}
