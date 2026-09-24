import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

Flow {
    id: actions
    property bool active: false
    property bool compact: false
    property bool busy: false
    property bool writing: false
    property bool upgradable: false
    property int selectedCount: 0
    property int uncheckedCount: 0
    property bool failed: false
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
        + (failureHint.visible ? failureHint.implicitWidth + spacing : 0)
    Controls.Button {
        id: upgradeButton
        objectName: "upgradeAllButton"
        visible: actions.selectedCount > 0
        text: actions.uncheckedCount === 0 ? "Update all" : (actions.compact ? "Update" : "Update selected (" + actions.selectedCount + ")")
        Accessible.name: actions.uncheckedCount === 0 ? "Update all" : "Update selected (" + actions.selectedCount + ")"
        enabled: !actions.busy && (actions.uncheckedCount > 0 || actions.upgradable)
        implicitHeight: Math.max(38, actions.textFont.pointSize * 3)
        onClicked: actions.upgradeRequested()
        background: Rectangle {
            radius: 7
            color: parent.enabled ? actions.accent : actions.surface
            border.color: parent.activeFocus ? actions.accent : actions.line
            border.width: parent.activeFocus ? 2 : 1
        }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon { name: "updates"; ink: upgradeButton.enabled ? actions.onAccent : actions.muted; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
            Text { text: upgradeButton.text; color: upgradeButton.enabled ? actions.onAccent : actions.muted; font: actions.textFont }
        }
    }
    Controls.Button {
        id: selectNoneButton
        objectName: "selectNoneButton"
        visible: actions.selectedCount > 0
        text: "Select none"
        Accessible.name: text
        enabled: !actions.writing
        implicitHeight: Math.max(38, actions.textFont.pointSize * 3)
        onClicked: actions.selectNoneRequested()
        background: Rectangle { radius: 7; color: parent.hovered ? actions.selection : actions.surface; border.color: parent.activeFocus ? actions.accent : actions.line; border.width: parent.activeFocus ? 2 : 1 }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon { name: "cancel"; ink: actions.ink; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
            Text { text: "Select none"; color: actions.ink; font: actions.textFont }
        }
    }
    Controls.Button {
        id: selectAllButton
        objectName: "selectAllButton"
        visible: actions.uncheckedCount > 0
        text: "Select all"
        Accessible.name: text
        enabled: !actions.writing
        implicitHeight: Math.max(38, actions.textFont.pointSize * 3)
        onClicked: actions.selectAllRequested()
        background: Rectangle { radius: 7; color: parent.hovered ? actions.selection : actions.surface; border.color: parent.activeFocus ? actions.accent : actions.line; border.width: parent.activeFocus ? 2 : 1 }
        contentItem: RowLayout {
            spacing: 8
            DeckIcon { name: "installed"; ink: actions.ink; Layout.preferredWidth: 18; Layout.preferredHeight: 18 }
            Text { text: "Select all"; color: actions.ink; font: actions.textFont }
        }
    }
    Controls.Label {
        id: failureHint
        objectName: "upgradeAllHint"
        visible: !actions.upgradable && !actions.busy && actions.failed
        text: "Update all is unavailable while a source has failed."
        color: actions.muted
        font.pointSize: actions.textFont.pointSize * 0.9
        wrapMode: Text.WordWrap
    }
}
