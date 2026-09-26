import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// A small floating message: a rounded card with an icon, the text, an
// optional action such as "Undo" and a close button. show() raises it; it
// hides itself after `timeout` ms, but never while the pointer is over it.
// Place it near the bottom of the window, horizontally centred; it slides up
// and fades in, and fades out when it leaves.
Item {
    id: toast
    objectName: "toast"
    property string text: ""
    property string actionText: ""
    // "info" | "success" | "danger" | "warning"
    property string tone: "info"
    property int timeout: 5000
    property bool open: false
    readonly property bool hovered: hover.hovered
    readonly property color toneColor: tone === "success" ? Theme.success
        : tone === "danger" ? Theme.danger
        : tone === "warning" ? Theme.warning : Theme.accent
    readonly property string toneIcon: tone === "success" ? "installed"
        : tone === "danger" || tone === "warning" ? "warning" : "info"
    signal actionTriggered()
    signal dismissed()

    function show(message, action, kind) {
        text = message || "";
        actionText = action || "";
        tone = kind || "info";
        open = true;
        hideTimer.restart();
    }
    // Hides the toast; dismissed() follows whether it timed out, was closed
    // or its action ran.
    function hide() {
        if (!open)
            return;
        open = false;
        hideTimer.stop();
        dismissed();
    }

    implicitWidth: Math.min(card.implicitWidth, 560)
    implicitHeight: card.implicitHeight
    width: implicitWidth
    height: implicitHeight
    visible: open || card.opacity > 0
    z: 100

    Accessible.role: Accessible.AlertMessage
    Accessible.name: text

    Timer {
        id: hideTimer
        objectName: "toastTimer"
        interval: toast.timeout
        running: false
        onTriggered: {
            if (toast.hovered)
                restart();
            else
                toast.hide();
        }
    }
    onHoveredChanged: if (open && !hovered) hideTimer.restart()

    Rectangle {
        id: card
        objectName: "toastCard"
        width: parent.width
        implicitWidth: row.implicitWidth + 2 * Theme.spacing + 6
        implicitHeight: Math.max(Theme.controlHeight + 8, row.implicitHeight + 2 * Theme.spacing)
        radius: Theme.cardRadius
        color: Theme.surface
        border.color: Theme.strongLine
        opacity: toast.open ? 1 : 0
        y: toast.open ? 0 : (Theme.motionEnabled ? 12 : 0)
        Behavior on opacity {
            enabled: Theme.motionEnabled
            NumberAnimation { duration: toast.open ? Theme.revealDuration : Theme.layoutDuration; easing.type: Easing.OutCubic }
        }
        Behavior on y {
            enabled: Theme.motionEnabled
            NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
        }
        // A soft drop shadow without effects modules.
        Rectangle {
            z: -1
            anchors.fill: parent
            anchors.topMargin: 3
            anchors.bottomMargin: -3
            radius: parent.radius
            color: Qt.rgba(0, 0, 0, Theme.dark ? 0.35 : 0.1)
        }
        HoverHandler { id: hover }

        RowLayout {
            id: row
            anchors.fill: parent
            anchors.leftMargin: Theme.spacing + 4
            anchors.rightMargin: 6
            spacing: Theme.spacing
            DeckIcon {
                name: toast.toneIcon
                ink: toast.toneColor
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
            }
            Controls.Label {
                objectName: "toastText"
                text: toast.text
                textFormat: Text.PlainText
                color: Theme.ink
                wrapMode: Text.Wrap
                maximumLineCount: 3
                elide: Text.ElideRight
                Layout.fillWidth: true
                Layout.maximumWidth: 400
            }
            Controls.Button {
                id: actionButton
                objectName: "toastAction"
                visible: toast.actionText.length > 0
                text: toast.actionText
                Accessible.name: text
                implicitHeight: Math.round(Theme.controlHeight * 0.85)
                horizontalPadding: Theme.spacing
                onClicked: {
                    toast.actionTriggered();
                    toast.hide();
                }
                background: Rectangle {
                    radius: Theme.controlRadius
                    color: actionButton.down ? Theme.tint(Theme.accent, 0.24) : actionButton.hovered ? Theme.selection : "transparent"
                    border.color: actionButton.visualFocus ? Theme.accent : "transparent"
                    border.width: 2
                    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
                }
                contentItem: Text {
                    text: actionButton.text
                    color: Theme.accent
                    font.weight: Font.DemiBold
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                }
            }
            ClearFieldButton {
                objectName: "toastClose"
                clearLabel: "Dismiss"
                onClicked: toast.hide()
            }
        }
    }
}
