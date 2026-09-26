import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// Shared checkbox indicator: an outlined box that fills with the accent
// when checked. The source checklist, Updates multi-select, and filters
// all use it so they stay visually identical.
Rectangle {
    required property bool ticked
    implicitWidth: 18
    implicitHeight: 18
    radius: 5
    color: ticked ? Theme.accent : "transparent"
    border.color: ticked ? Theme.accent : Theme.strongLine
    border.width: 1.5
    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
    Behavior on border.color { ColorAnimation { duration: Theme.feedbackDuration } }
    DeckIcon {
        name: "installed"
        ink: Theme.accentInk
        anchors.centerIn: parent
        width: 13
        height: 13
        opacity: ticked ? 1 : 0
        scale: ticked ? 1 : 0.6
        Behavior on opacity { NumberAnimation { duration: Theme.feedbackDuration } }
        Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutBack } }
    }
}
