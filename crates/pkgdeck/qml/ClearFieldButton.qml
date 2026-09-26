import QtQuick
import QtQuick.Controls as Controls

Controls.ToolButton {
    id: button
    property color ink: Theme.muted
    property color hoverColor: Theme.hoverTint
    property string clearLabel: "Clear text"

    implicitWidth: 32
    implicitHeight: 32
    padding: 8
    Accessible.name: clearLabel
    Controls.ToolTip.visible: hovered
    Controls.ToolTip.delay: 500
    Controls.ToolTip.text: clearLabel
    background: Rectangle {
        radius: Theme.smallRadius
        color: button.hovered || button.down ? button.hoverColor : "transparent"
        border.color: button.visualFocus ? Theme.accent : "transparent"
        border.width: 2
        Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
    }
    contentItem: DeckIcon {
        name: "cancel"
        ink: button.ink
    }
}
