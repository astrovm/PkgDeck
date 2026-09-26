import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

// Details for the selected row. The header (icon, name, source, installed
// state, the row's action and Close) follows the selection at once; the
// body shows a skeleton until the details for that same row arrive.
Rectangle {
    id: panel
    objectName: "detailsPanel"
    property var selected: null
    property string selectionIdentity: ""
    property var screenshots: []
    property string description: ""
    property var detailsData: ({})
    // Plain-text summary of the facts below; kept for callers that test
    // whether there is anything beyond the description.
    readonly property string metadataText: detailMatchesSelection ? [
        detailsData.publisher ? "Publisher: " + detailsData.publisher : "",
        detailsData.license ? "License: " + detailsData.license : "",
        detailsData.homepage ? "Homepage: " + detailsData.homepage : "",
        (detailsData.dependencies || []).length ? "Dependencies: " + detailsData.dependencies.join(", ") : ""
    ].filter(Boolean).join("\n") : ""
    property string iconSource: ""
    property bool compact: false
    property bool motionEnabled: Theme.motionEnabled
    property bool detailMatchesSelection: false
    // True when the selected package is installed; shows an "Installed" chip.
    property bool installed: false
    // The row's own action, repeated in the header. Empty text hides it.
    property string actionText: ""
    property string actionSymbol: "install"       // DeckIcon: install | remove | updates
    property string actionTone: "accent"          // success | danger | accent
    property bool actionEnabled: true
    // Maps a raw source id to its display name for the header.
    property var sourceName: (id) => id
    property color canvas: Theme.canvas
    property color surface: Theme.surface
    property color ink: Theme.ink
    property color muted: Theme.muted
    property color line: Theme.line
    property color accent: Theme.accent
    property font textFont: Theme.baseFont
    property alias detailsContentItem: detailsContent
    // A package whose details have not arrived yet shows the skeleton.
    readonly property bool loading: selected !== null && selected !== undefined
        && selected.kind === "package" && !detailMatchesSelection
    // Anything to show beyond the header, once details are in.
    readonly property bool hasDetails: detailMatchesSelection
        && (description.length > 0 || screenshots.length > 0 || metadataText.length > 0)
    readonly property real contentIdealHeight: Math.max(88, 2 * Theme.gutter + header.implicitHeight
        + detailsContent.spacing + detailBody.implicitHeight)
    // Never collapses while the next row loads: it keeps the last settled
    // height (or the skeleton's, if taller) so the panel does not jump.
    readonly property real idealHeight: loading ? Math.max(settledHeight, contentIdealHeight) : contentIdealHeight
    property real settledHeight: 0
    property bool dependenciesExpanded: false
    readonly property int thumbnailWidth: 230
    readonly property int thumbnailHeight: 130
    signal closeRequested()
    signal screenshotRequested(string url, string caption)
    signal screenshotFailed(string url, string identity)
    signal actionRequested()

    onContentIdealHeightChanged: if (!loading && selected) settledHeight = contentIdealHeight
    onLoadingChanged: if (!loading && selected) settledHeight = contentIdealHeight
    onSelectedChanged: if (!selected) settledHeight = 0
    onSelectionIdentityChanged: dependenciesExpanded = false

    readonly property color actionColor: actionTone === "success" ? Theme.success
        : actionTone === "danger" ? Theme.danger : accent
    readonly property string homepage: detailMatchesSelection && detailsData.homepage ? String(detailsData.homepage) : ""
    readonly property bool homepageOpens: /^https?:\/\//i.test(homepage)
    readonly property var dependencies: detailMatchesSelection ? (detailsData.dependencies || []) : []
    readonly property var facts: detailMatchesSelection ? [
        {label: "Publisher", value: detailsData.publisher || ""},
        {label: "License", value: detailsData.license || ""}
    ].filter(fact => fact.value) : []
    // The source's display name, under the package name.
    readonly property string subtitle: selected && selected.kind === "package" && selected.source
        ? String(sourceName(selected.source) || selected.source) : ""

    color: surface
    radius: Theme.cardRadius
    border.color: line

    // Keyboard focus rings only; a mouse click leaves none.
    component FocusFrame: Rectangle {
        property bool shown: false
        anchors.fill: parent
        anchors.margins: -2
        radius: Theme.controlRadius + 2
        color: "transparent"
        border.color: panel.accent
        border.width: 2
        visible: shown
    }

    ColumnLayout {
        id: detailsContent
        objectName: "detailsContent"
        anchors.fill: parent
        anchors.margins: Theme.gutter
        spacing: Theme.spacingSmall
        RowLayout {
            id: header
            objectName: "detailsHeader"
            Layout.fillWidth: true
            spacing: Theme.spacing
            Item {
                objectName: "detailsIcon"
                Layout.preferredWidth: 44
                Layout.preferredHeight: 44
                Layout.alignment: Qt.AlignVCenter
                Rectangle {
                    anchors.fill: parent
                    radius: Theme.controlRadius
                    color: Theme.tint(panel.accent, 0.1)
                    visible: detailIcon.status !== Image.Ready
                    DeckIcon {
                        name: panel.selected ? (panel.selected.kind === "source" ? panel.selected.source : (panel.selected.kind === "failure" ? "warning" : "package")) : "package"
                        ink: panel.accent
                        anchors.centerIn: parent
                        width: 24
                        height: 24
                    }
                }
                Image {
                    id: detailIcon
                    anchors.fill: parent
                    visible: status === Image.Ready
                    asynchronous: true
                    source: panel.iconSource
                    sourceSize.width: Math.round(44 * Screen.devicePixelRatio)
                    sourceSize.height: Math.round(44 * Screen.devicePixelRatio)
                    fillMode: Image.PreserveAspectFit
                    Accessible.ignored: true
                }
                // Where the package comes from, on the icon's corner.
                Rectangle {
                    objectName: "detailsSourceBadge"
                    visible: !!panel.selected && panel.selected.kind === "package" && !!panel.selected.source
                    width: 20
                    height: 20
                    radius: 10
                    x: parent.width - width + 4
                    y: parent.height - height + 4
                    color: panel.surface
                    border.color: panel.line
                    DeckIcon {
                        anchors.centerIn: parent
                        width: 13
                        height: 13
                        name: panel.selected && panel.selected.source ? panel.selected.source : "package"
                        ink: panel.muted
                    }
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignVCenter
                spacing: 2
                Controls.Label {
                    objectName: "detailsTitle"
                    text: panel.selected ? (panel.selected.display_name || panel.selected.name || "") : ""
                    textFormat: Text.PlainText
                    color: panel.ink
                    font.pointSize: Theme.pointSize(Theme.titleScale)
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                }
                RowLayout {
                    spacing: Theme.spacingSmall
                    Layout.fillWidth: true
                    visible: panel.subtitle.length > 0 || panel.installed
                    Controls.Label {
                        objectName: "detailsSubtitle"
                        visible: text.length > 0
                        text: panel.subtitle
                        textFormat: Text.PlainText
                        color: panel.muted
                        font.pointSize: Theme.pointSize(Theme.smallScale)
                        elide: Text.ElideRight
                        Layout.maximumWidth: implicitWidth
                        Layout.fillWidth: true
                    }
                    Controls.Label {
                        objectName: "installedChip"
                        visible: panel.installed
                        text: "Installed"
                        color: Theme.success
                        font.pointSize: Theme.pointSize(Theme.captionScale)
                        font.weight: Font.DemiBold
                        leftPadding: 7
                        rightPadding: 7
                        topPadding: 1
                        bottomPadding: 1
                        background: Rectangle { radius: height / 2; color: Theme.tint(Theme.success, 0.14) }
                    }
                    Item { Layout.fillWidth: true }
                }
            }
            Controls.Button {
                id: actionButton
                objectName: "detailsActionButton"
                visible: panel.actionText.length > 0
                enabled: panel.actionEnabled
                text: panel.compact ? "" : panel.actionText
                Accessible.name: panel.actionText + (panel.selected ? " " + (panel.selected.display_name || panel.selected.name || "") : "")
                Controls.ToolTip.visible: hovered && panel.compact
                Controls.ToolTip.delay: 500
                Controls.ToolTip.text: panel.actionText
                implicitHeight: Theme.controlHeight
                horizontalPadding: panel.compact ? 9 : 14
                Layout.alignment: Qt.AlignVCenter
                onClicked: panel.actionRequested()
                scale: down && panel.motionEnabled ? 0.97 : 1
                Behavior on scale { NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic } }
                background: Rectangle {
                    radius: Theme.controlRadius
                    color: !actionButton.enabled ? "transparent"
                        : Theme.tint(panel.actionColor, actionButton.down ? 0.26 : actionButton.hovered ? 0.2 : 0.13)
                    border.color: actionButton.enabled ? Theme.tint(panel.actionColor, 0.4) : panel.line
                    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
                    FocusFrame { shown: actionButton.visualFocus }
                }
                contentItem: RowLayout {
                    spacing: 8
                    DeckIcon {
                        name: panel.actionSymbol
                        ink: actionButton.enabled ? panel.actionColor : panel.muted
                        Layout.preferredWidth: 18
                        Layout.preferredHeight: 18
                        Layout.alignment: Qt.AlignCenter
                    }
                    Text {
                        visible: text.length > 0
                        text: actionButton.text
                        color: actionButton.enabled ? panel.actionColor : panel.muted
                        font.family: panel.textFont.family
                        font.pointSize: panel.textFont.pointSize
                        font.weight: Font.DemiBold
                    }
                }
            }
            Controls.Button {
                id: closeButton
                objectName: "closeDetailsButton"
                Accessible.name: "Close details"
                Controls.ToolTip.visible: hovered
                Controls.ToolTip.delay: 500
                Controls.ToolTip.text: Accessible.name
                Layout.preferredWidth: 38
                Layout.alignment: Qt.AlignVCenter
                implicitHeight: 38
                onClicked: panel.closeRequested()
                background: Rectangle {
                    color: closeButton.down ? Theme.tint(panel.ink, 0.12) : closeButton.hovered ? Theme.tint(panel.ink, 0.08) : "transparent"
                    radius: Theme.controlRadius
                    border.color: closeButton.visualFocus ? panel.accent : "transparent"
                    border.width: 2
                    Behavior on color { ColorAnimation { duration: Theme.feedbackDuration } }
                }
                // A content item fills the button; keep the icon at its size.
                contentItem: Item {
                    DeckIcon { objectName: "closeDetailsIcon"; anchors.centerIn: parent; name: "cancel"; ink: panel.muted; width: 16; height: 16 }
                }
            }
        }
        DeckScrollView {
            ink: panel.muted
            id: detailScroll
            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true
            ColumnLayout {
                id: detailBody
                width: detailScroll.availableWidth
                spacing: Theme.spacing
                // Placeholder bars while the selected row's details load.
                Column {
                    id: skeleton
                    objectName: "detailsSkeleton"
                    visible: panel.loading
                    Layout.fillWidth: true
                    Layout.topMargin: 4
                    spacing: 10
                    readonly property bool pulsing: pulse.running
                    Repeater {
                        model: [0.92, 1, 0.78, 0.46]
                        Rectangle {
                            required property real modelData
                            width: skeleton.width * modelData
                            height: 10
                            radius: 5
                            color: Theme.tint(panel.ink, 0.09)
                        }
                    }
                    SequentialAnimation on opacity {
                        id: pulse
                        running: skeleton.visible && panel.motionEnabled && Theme.pulseDuration > 0
                        loops: Animation.Infinite
                        NumberAnimation { from: 1; to: 0.45; duration: Math.max(1, Theme.pulseDuration); easing.type: Easing.InOutSine }
                        NumberAnimation { from: 0.45; to: 1; duration: Math.max(1, Theme.pulseDuration); easing.type: Easing.InOutSine }
                        onRunningChanged: if (!running) skeleton.opacity = 1
                    }
                }
                ColumnLayout {
                    id: loadedBody
                    objectName: "detailsBody"
                    visible: !panel.loading
                    Layout.fillWidth: true
                    spacing: Theme.spacing
                    // Details fade in when they arrive.
                    opacity: panel.loading ? 0 : 1
                    Behavior on opacity {
                        enabled: panel.motionEnabled
                        NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                    }
                    ListView {
                        id: screenshotGallery
                        objectName: "screenshotGallery"
                        visible: panel.screenshots.length > 0
                        Layout.fillWidth: true
                        Layout.preferredHeight: panel.compact ? 72 : panel.thumbnailHeight
                        orientation: ListView.Horizontal
                        spacing: Theme.spacing
                        clip: true
                        model: panel.screenshots
                        cacheBuffer: 0
                        reuseItems: true
                        delegate: Controls.AbstractButton {
                            id: thumbnail
                            required property var modelData
                            width: panel.compact ? 128 : panel.thumbnailWidth
                            height: screenshotGallery.height
                            enabled: preview.status === Image.Ready
                            Accessible.name: modelData.caption || "View app screenshot"
                            Controls.ToolTip.visible: hovered
                            Controls.ToolTip.delay: 500
                            Controls.ToolTip.text: Accessible.name
                            onClicked: panel.screenshotRequested(modelData.url, modelData.caption || "")
                            background: Rectangle {
                                color: panel.canvas
                                radius: Theme.controlRadius
                                border.color: thumbnail.visualFocus ? panel.accent : thumbnail.hovered ? Theme.strongLine : panel.line
                                border.width: thumbnail.visualFocus ? 2 : 1
                            }
                            contentItem: Item {
                                Image {
                                    id: preview
                                    onStatusChanged: {
                                        if (status === Image.Error)
                                            Qt.callLater(panel.screenshotFailed, thumbnail.modelData.url, panel.selectionIdentity);
                                    }
                                    anchors.fill: parent
                                    anchors.margins: 1
                                    source: panel.detailMatchesSelection ? thumbnail.modelData.url : ""
                                    asynchronous: true
                                    cache: true
                                    // Decode at the largest thumbnail size shown, not the
                                    // full image. A fixed size keeps a compact/wide switch
                                    // from reloading every image.
                                    sourceSize.width: Math.round(panel.thumbnailWidth * Screen.devicePixelRatio)
                                    sourceSize.height: Math.round(panel.thumbnailHeight * Screen.devicePixelRatio)
                                    fillMode: Image.PreserveAspectFit
                                    opacity: status === Image.Ready ? 1 : 0
                                    Behavior on opacity {
                                        enabled: panel.motionEnabled
                                        NumberAnimation { duration: Theme.revealDuration }
                                    }
                                }
                                // A soft placeholder while the image loads.
                                Rectangle {
                                    id: previewPlaceholder
                                    anchors.fill: parent
                                    anchors.margins: 6
                                    radius: Theme.smallRadius
                                    color: Theme.tint(panel.ink, 0.06)
                                    visible: preview.status === Image.Loading
                                    SequentialAnimation on opacity {
                                        running: previewPlaceholder.visible && panel.motionEnabled && Theme.pulseDuration > 0
                                        loops: Animation.Infinite
                                        NumberAnimation { from: 1; to: 0.5; duration: Math.max(1, Theme.pulseDuration) }
                                        NumberAnimation { from: 0.5; to: 1; duration: Math.max(1, Theme.pulseDuration) }
                                    }
                                }
                            }
                        }
                    }
                    TextEdit {
                        objectName: "packageDetails"
                        visible: panel.description.length > 0
                        Layout.fillWidth: true
                        readOnly: true
                        selectByMouse: true
                        color: panel.ink
                        opacity: 0.86
                        padding: 0
                        font.pointSize: panel.textFont.pointSize
                        wrapMode: TextEdit.Wrap
                        textFormat: TextEdit.PlainText
                        text: panel.description
                        Accessible.name: "Package details"
                    }
                    // Facts as a two-column grid, then the dependency list.
                    ColumnLayout {
                        objectName: "packageMetadata"
                        // The same facts as plain text, for tests and copying.
                        readonly property string text: panel.metadataText
                        visible: panel.metadataText.length > 0
                        Layout.fillWidth: true
                        Layout.topMargin: panel.description.length > 0 || panel.screenshots.length > 0 ? 2 : 0
                        spacing: Theme.spacing
                        Accessible.name: "Additional package metadata"
                        GridLayout {
                            objectName: "detailsFacts"
                            visible: panel.facts.length > 0 || panel.homepage.length > 0
                            columns: 2
                            columnSpacing: Theme.spacingLarge
                            rowSpacing: Theme.spacingSmall
                            Layout.fillWidth: true
                            Repeater {
                                model: panel.facts
                                delegate: Controls.Label {
                                    required property var modelData
                                    required property int index
                                    Layout.row: index
                                    Layout.column: 0
                                    Layout.alignment: Qt.AlignTop
                                    text: modelData.label
                                    color: panel.muted
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                }
                            }
                            Repeater {
                                model: panel.facts
                                delegate: TextEdit {
                                    required property var modelData
                                    required property int index
                                    Layout.row: index
                                    Layout.column: 1
                                    Layout.fillWidth: true
                                    text: modelData.value
                                    textFormat: TextEdit.PlainText
                                    readOnly: true
                                    selectByMouse: true
                                    wrapMode: TextEdit.Wrap
                                    color: panel.ink
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                    Accessible.name: modelData.label + ": " + modelData.value
                                }
                            }
                            Controls.Label {
                                visible: panel.homepage.length > 0
                                Layout.row: panel.facts.length
                                Layout.column: 0
                                Layout.alignment: Qt.AlignVCenter
                                text: "Homepage"
                                color: panel.muted
                                font.pointSize: Theme.pointSize(Theme.smallScale)
                            }
                            Controls.AbstractButton {
                                id: homepageLink
                                objectName: "homepageLink"
                                visible: panel.homepage.length > 0
                                enabled: panel.homepageOpens
                                Layout.row: panel.facts.length
                                Layout.column: 1
                                Layout.fillWidth: true
                                Layout.maximumWidth: implicitWidth
                                hoverEnabled: true
                                Accessible.role: Accessible.Link
                                Accessible.name: "Homepage " + panel.homepage
                                Controls.ToolTip.visible: hovered
                                Controls.ToolTip.delay: 500
                                Controls.ToolTip.text: panel.homepage
                                onClicked: Qt.openUrlExternally(panel.homepage)
                                HoverHandler { cursorShape: homepageLink.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor }
                                background: FocusFrame { shown: homepageLink.visualFocus; radius: Theme.smallRadius }
                                contentItem: RowLayout {
                                    spacing: 5
                                    Text {
                                        text: panel.homepage.replace(/^https?:\/\//i, "").replace(/\/$/, "")
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                        color: homepageLink.enabled ? panel.accent : panel.ink
                                        font.pointSize: Theme.pointSize(Theme.smallScale)
                                        font.underline: homepageLink.hovered && homepageLink.enabled
                                        Layout.fillWidth: true
                                    }
                                    DeckIcon {
                                        visible: homepageLink.enabled
                                        name: "external"
                                        ink: panel.accent
                                        Layout.preferredWidth: 13
                                        Layout.preferredHeight: 13
                                    }
                                }
                            }
                        }
                        // Collapsed by default: long APT lists stay out of the way.
                        Controls.AbstractButton {
                            id: dependenciesToggle
                            objectName: "dependenciesToggle"
                            visible: panel.dependencies.length > 0
                            Layout.fillWidth: true
                            hoverEnabled: true
                            Accessible.role: Accessible.Button
                            Accessible.name: (panel.dependenciesExpanded ? "Hide " : "Show ") + "dependencies (" + panel.dependencies.length + ")"
                            onClicked: panel.dependenciesExpanded = !panel.dependenciesExpanded
                            background: FocusFrame { shown: dependenciesToggle.visualFocus; radius: Theme.smallRadius }
                            contentItem: RowLayout {
                                spacing: 6
                                DeckIcon {
                                    name: "right"
                                    ink: panel.muted
                                    rotation: panel.dependenciesExpanded ? 90 : 0
                                    Layout.preferredWidth: 13
                                    Layout.preferredHeight: 13
                                    Behavior on rotation {
                                        enabled: panel.motionEnabled
                                        NumberAnimation { duration: Theme.feedbackDuration; easing.type: Easing.OutCubic }
                                    }
                                }
                                Text {
                                    objectName: "dependenciesHeading"
                                    text: "Dependencies (" + panel.dependencies.length + ")"
                                    color: dependenciesToggle.hovered ? panel.ink : panel.muted
                                    font.pointSize: Theme.pointSize(Theme.smallScale)
                                    font.weight: Font.DemiBold
                                    Layout.fillWidth: true
                                }
                            }
                        }
                        Flow {
                            id: dependencyChips
                            objectName: "dependencyChips"
                            visible: panel.dependenciesExpanded && panel.dependencies.length > 0
                            Layout.fillWidth: true
                            spacing: 6
                            opacity: visible ? 1 : 0
                            Behavior on opacity {
                                enabled: panel.motionEnabled
                                NumberAnimation { duration: Theme.revealDuration; easing.type: Easing.OutCubic }
                            }
                            Repeater {
                                model: dependencyChips.visible ? panel.dependencies : []
                                delegate: Controls.Label {
                                    required property var modelData
                                    text: String(modelData)
                                    textFormat: Text.PlainText
                                    color: panel.ink
                                    font.pointSize: Theme.pointSize(Theme.captionScale)
                                    elide: Text.ElideRight
                                    width: Math.min(implicitWidth, dependencyChips.width)
                                    leftPadding: 8
                                    rightPadding: 8
                                    topPadding: 3
                                    bottomPadding: 3
                                    background: Rectangle {
                                        radius: Theme.smallRadius
                                        color: Theme.tint(panel.ink, 0.06)
                                        border.color: panel.line
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
