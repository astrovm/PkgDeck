import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// Grouped settings section with a title.
Rectangle {
    id: card
    property string title: ""
    default property alias content: cardBody.data
    Layout.fillWidth: true
    Layout.maximumWidth: 760
    implicitHeight: cardColumn.implicitHeight + 32
    color: Theme.surface
    radius: Theme.cardRadius
    border.color: Theme.line
    ColumnLayout {
        id: cardColumn
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 16
        spacing: 10
        Controls.Label {
            text: card.title
            visible: text.length > 0
            color: Theme.muted
            font.pointSize: Theme.baseFont.pointSize * 0.85
            font.weight: Font.DemiBold
            font.capitalization: Font.AllUppercase
            font.letterSpacing: 0.6
        }
        ColumnLayout {
            id: cardBody
            Layout.fillWidth: true
            spacing: 8
        }
    }
}
