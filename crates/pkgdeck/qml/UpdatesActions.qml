import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Flow {
    id: actions
    property bool active: false
    property bool compact: false
    // Very narrow pages keep only the selection buttons' icons.
    property bool narrow: false
    property bool busy: false
    property bool writing: false
    property int selectedCount: 0
    property int uncheckedCount: 0
    property color surface: Theme.surface
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color line: Theme.line
    property color accent: Theme.accent
    property color onAccent: Theme.accentInk
    property color selection: Theme.selection
    property font textFont: Theme.baseFont
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
        Controls.ToolTip.visible: hovered && text.length === 0
        Controls.ToolTip.delay: 500
        Controls.ToolTip.text: Accessible.name
        implicitWidth: contentItem.implicitWidth + leftPadding + rightPadding
        implicitHeight: Theme.controlHeight
        horizontalPadding: 14
        scale: down && Theme.motionEnabled ? 0.97 : 1
        Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
        background: Rectangle {
            radius: Theme.controlRadius
            color: button.primary
                ? (!button.enabled ? actions.surface : button.down ? Qt.darker(actions.accent, 1.08) : button.hovered ? Qt.lighter(actions.accent, 1.08) : actions.accent)
                : (button.down ? Theme.tint(actions.accent, 0.24) : button.hovered ? actions.selection : actions.surface)
            // The ring marks keyboard focus only; a click leaves none.
            border.color: button.visualFocus ? (button.primary ? actions.ink : actions.accent)
                : (button.primary && button.enabled ? "transparent" : actions.line)
            border.width: button.visualFocus ? 2 : 1
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
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
                visible: text.length > 0
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
        text: actions.narrow ? "" : "Select none"
        Accessible.name: "Select none"
        enabled: !actions.writing
        onClicked: actions.selectNoneRequested()
    }
    Action {
        id: selectAllButton
        objectName: "selectAllButton"
        symbol: "installed"
        visible: actions.uncheckedCount > 0
        text: actions.narrow ? "" : "Select all"
        Accessible.name: "Select all"
        enabled: !actions.writing
        onClicked: actions.selectAllRequested()
    }
}
