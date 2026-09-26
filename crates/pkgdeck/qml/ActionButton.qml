import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Controls.Button {
    id: control
    SystemPalette { id: disabledPalette; colorGroup: SystemPalette.Disabled }
    property color glyphColor: Theme.ink
    property bool primary: false
    // A primary button's fill and label; destructive actions use the danger colour.
    property color primaryColor: Theme.accent
    property color primaryInk: Theme.accentInk
    // Sidebar entry: borderless, with `current` marking the open page.
    property bool navigation: false
    property bool current: false
    // Sidebar entries left-align their label; `centered` centers an icon-only entry.
    property bool centered: false
    // `flat` (from Button) marks borderless actions tinted by their glyph on hover.
    property string symbol: "package"
    property url iconSource: ""
    property string tooltipText: ""
    // Underlines this character of the label as its Alt shortcut.
    property int mnemonicIndex: -1
    readonly property bool systemDisabled: !enabled && Theme.systemAppearance && !flat && !navigation
    Accessible.name: text
    Controls.ToolTip.visible: hovered && tooltipText.length > 0
    Controls.ToolTip.delay: 500
    Controls.ToolTip.text: tooltipText
    implicitHeight: Theme.controlHeight
    horizontalPadding: text.length > 0 ? 14 : 9
    verticalPadding: 4
    opacity: enabled || Theme.systemAppearance ? 1 : 0.42
    scale: down ? 0.97 : 1
    Behavior on opacity { NumberAnimation { duration: Theme.feedbackDuration } }
    Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
    background: Rectangle {
        radius: Theme.controlRadius
        Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        color: control.systemDisabled ? disabledPalette.button
            : control.primary && control.enabled ? (control.down ? Qt.darker(control.primaryColor, 1.08) : control.hovered ? Qt.lighter(control.primaryColor, 1.08) : control.primaryColor)
            : control.navigation ? (control.current ? Theme.selection : control.hovered ? Theme.hoverTint : "transparent")
            : control.flat ? (control.enabled && control.hovered ? Theme.tint(control.glyphColor, 0.14) : "transparent")
            : Theme.surface
        border.color: control.visualFocus ? Theme.accent : control.systemDisabled ? disabledPalette.mid
            : (control.navigation || control.flat || control.primary) ? "transparent" : Theme.line
        border.width: control.visualFocus ? 2 : 1
        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            visible: !control.primary && !control.navigation && !control.flat
            color: control.enabled && control.down ? Theme.tint(Theme.ink, 0.1) : control.enabled && control.hovered ? Theme.hoverTint : "transparent"
            Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
        }
    }
    contentItem: Item {
        implicitWidth: buttonContents.implicitWidth
        implicitHeight: buttonContents.implicitHeight
        RowLayout {
            id: buttonContents
            x: control.navigation && !control.centered ? 0 : (parent.width - width) / 2
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, parent.width)
            height: implicitHeight
            spacing: 9
            DeckIcon {
                name: control.symbol
                visible: control.symbol.length > 0
                ink: control.systemDisabled ? disabledPalette.buttonText : !control.enabled && control.flat ? Theme.muted : control.primary && control.enabled ? control.primaryInk
                    : control.navigation && control.current ? Theme.accent : control.glyphColor
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                Layout.alignment: Qt.AlignVCenter
                Behavior on ink { ColorAnimation { duration: Theme.feedbackDuration } }
            }
            Image {
                objectName: control.iconSource.toString().length > 0 ? control.objectName + "Icon" : ""
                source: control.iconSource
                visible: control.iconSource.toString().length > 0
                sourceSize.width: Math.ceil(width * Screen.devicePixelRatio)
                sourceSize.height: Math.ceil(height * Screen.devicePixelRatio)
                fillMode: Image.PreserveAspectFit
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                Layout.alignment: Qt.AlignVCenter
                Accessible.ignored: true
            }
            Text {
                text: control.mnemonicIndex >= 0 && control.mnemonicIndex < control.text.length
                    ? control.text.slice(0, control.mnemonicIndex) + "<u>" + control.text.charAt(control.mnemonicIndex) + "</u>" + control.text.slice(control.mnemonicIndex + 1)
                    : control.text
                textFormat: control.mnemonicIndex >= 0 ? Text.StyledText : Text.PlainText
                visible: text.length > 0
                font.family: control.font.family
                font.pointSize: control.font.pointSize
                font.weight: control.primary || (control.navigation && control.current) ? Font.DemiBold : Font.Normal
                color: control.systemDisabled ? disabledPalette.buttonText : !control.enabled && control.flat ? Theme.muted : control.primary && control.enabled ? control.primaryInk : Theme.ink
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                Layout.alignment: Qt.AlignVCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
        }
    }
}
