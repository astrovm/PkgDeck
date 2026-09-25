import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Flow {
    id: actions
    property bool active: false
    property bool compact: false
    property bool busy: false
    property bool writing: false
    property int selectedCount: 0
    property int uncheckedCount: 0
    property color surface
    property color ink
    property color muted
    property color line
    property color accent
    property color onAccent
    property color selection
    property font textFont
    signal upgradeRequested()
    signal selectNoneRequested()
    signal selectAllRequested()

    visible: active
    spacing: 8
    readonly property real preferredWidth: (upgradeButton.visible ? upgradeButton.implicitWidth + spacing : 0)
        + (selectNoneButton.visible ? selectNoneButton.implicitWidth + spacing : 0)
        + (selectAllButton.visible ? selectAllButton.implicitWidth + spacing : 0)

    component Action: Controls.Button {
        id: button
        property bool primary: false
        property string symbol: ""
        Accessible.name: text
        implicitWidth: contentItem.implicitWidth + leftPadding + rightPadding
        implicitHeight: Math.max(36, Math.round(actions.textFont.pointSize * 2.9))
        horizontalPadding: 14
        scale: down ? 0.97 : 1
        Behavior on scale { NumberAnimation { duration: 120; easing.type: Easing.OutCubic } }
        background: Rectangle {
            radius: 9
            color: button.primary
                ? (!button.enabled ? actions.surface : button.down ? Qt.darker(actions.accent, 1.08) : button.hovered ? Qt.lighter(actions.accent, 1.08) : actions.accent)
                : (button.hovered ? actions.selection : actions.surface)
            border.color: button.activeFocus ? actions.accent : (button.primary && button.enabled ? "transparent" : actions.line)
            border.width: button.activeFocus ? 2 : 1
            Behavior on color { ColorAnimation { duration: 120 } }
        }
        contentItem: RowLayout {
            spacing: 9
            DeckIcon {
                name: button.symbol
                ink: button.primary ? (button.enabled ? actions.onAccent : actions.muted) : actions.ink
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
            }
            Text {
                text: button.text
                color: button.primary ? (button.enabled ? actions.onAccent : actions.muted) : actions.ink
                font.family: actions.textFont.family
                font.pointSize: actions.textFont.pointSize
                font.weight: button.primary ? Font.DemiBold : Font.Normal
            }
        }
    }

    Action {
        id: upgradeButton
        objectName: "upgradeAllButton"
        primary: true
        symbol: "updates"
        visible: actions.selectedCount > 0
        text: actions.uncheckedCount === 0 ? "Update all" : (actions.compact ? "Update" : "Update selected (" + actions.selectedCount + ")")
        Accessible.name: actions.uncheckedCount === 0 ? "Update all" : "Update selected (" + actions.selectedCount + ")"
        enabled: !actions.busy
        onClicked: actions.upgradeRequested()
    }
    Action {
        id: selectNoneButton
        objectName: "selectNoneButton"
        symbol: "cancel"
        visible: actions.selectedCount > 0
        text: "Select none"
        enabled: !actions.writing
        onClicked: actions.selectNoneRequested()
    }
    Action {
        id: selectAllButton
        objectName: "selectAllButton"
        symbol: "installed"
        visible: actions.uncheckedCount > 0
        text: "Select all"
        enabled: !actions.writing
        onClicked: actions.selectAllRequested()
    }
}
