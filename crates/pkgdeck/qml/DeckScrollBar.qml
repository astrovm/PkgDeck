import QtQuick
import QtQuick.Controls as Controls

// Only the handle. Desktop styles draw a groove with a hard separator line
// between the list and the scrollbar; this leaves the list edge clean.
// Like Breeze, the handle fades in while the view scrolls or the pointer is
// over it, and fades out shortly after it goes idle. With motion off it
// shows and hides without fading.
Controls.ScrollBar {
    id: bar
    property color ink: Theme.muted
    // Lets tests tell this apart from a style-drawn scrollbar.
    readonly property bool plainHandle: true
    // How long the handle stays after the last scroll or hover, in ms.
    property int lingerInterval: 900
    // True while the handle is on screen (or fading in).
    readonly property bool revealed: size < 1 && (active || hovered || pressed || linger.running)
    readonly property real handleOpacity: !revealed ? 0 : pressed ? 0.75 : hovered ? 0.6 : 0.42

    implicitWidth: 10
    implicitHeight: 10
    padding: 2
    minimumSize: 0.08
    hoverEnabled: true
    background: Item {}
    onActiveChanged: if (!active) linger.restart()
    onHoveredChanged: if (!hovered) linger.restart()
    onPositionChanged: if (size < 1) linger.restart()
    Timer { id: linger; interval: bar.lingerInterval }
    contentItem: Rectangle {
        implicitWidth: 6
        implicitHeight: 6
        radius: width / 2
        color: bar.ink
        opacity: bar.handleOpacity
        Behavior on opacity {
            enabled: Theme.motionEnabled
            NumberAnimation {
                duration: bar.revealed ? Theme.feedbackDuration : Theme.layoutDuration
                easing.type: Easing.OutCubic
            }
        }
    }
}
