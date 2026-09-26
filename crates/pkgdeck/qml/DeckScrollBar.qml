import QtQuick
import QtQuick.Controls as Controls

// Only the handle. Desktop styles draw a groove with a hard separator line
// between the list and the scrollbar; this leaves the list edge clean.
Controls.ScrollBar {
    id: bar
    property color ink

    implicitWidth: 10
    implicitHeight: 10
    padding: 2
    minimumSize: 0.08
    background: Item {}
    contentItem: Rectangle {
        implicitWidth: 6
        implicitHeight: 6
        radius: 3
        color: bar.ink
        opacity: bar.size >= 1 ? 0 : bar.pressed ? 0.7 : bar.hovered ? 0.55 : 0.35
        Behavior on opacity { NumberAnimation { duration: 120 } }
    }
}
