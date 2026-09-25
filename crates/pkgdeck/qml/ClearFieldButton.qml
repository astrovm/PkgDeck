import QtQuick
import QtQuick.Controls as Controls

Controls.ToolButton {
    id: button
    property color ink
    property color hoverColor
    property string clearLabel: "Clear text"

    implicitWidth: 32
    implicitHeight: 32
    padding: 8
    Accessible.name: clearLabel
    Controls.ToolTip.visible: hovered
    Controls.ToolTip.text: clearLabel
    background: Rectangle {
        radius: 6
        color: button.hovered ? button.hoverColor : "transparent"
    }
    contentItem: DeckIcon {
        name: "cancel"
        ink: button.ink
    }
}
