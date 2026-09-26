import QtQuick
import QtQuick.Controls as Controls

// A ScrollView with the plain DeckScrollBar. A replaced ScrollView scrollbar
// is not laid out by the style, so it is placed along the edges here.
Controls.ScrollView {
    id: view
    property color ink: Theme.muted
    // Content fits the width unless a wider contentWidth is set; only then
    // is there a horizontal handle. It overlays the bottom edge (and fades
    // when idle) so it never feeds back into the vertical layout.
    readonly property bool horizontallyScrollable: contentWidth > availableWidth + 0.5

    contentWidth: availableWidth
    clip: true
    // Keep content clear of the scrollbar while there is something to scroll.
    // Reads the heights, not the bar, so the padding never loops back on itself.
    rightPadding: contentHeight > availableHeight + 0.5 ? Theme.scrollGutter : 0
    Controls.ScrollBar.vertical: DeckScrollBar {
        id: verticalBar
        parent: view
        ink: view.ink
        x: view.mirrored ? 0 : view.width - width
        y: view.topPadding
        height: view.availableHeight
    }
    Controls.ScrollBar.horizontal: DeckScrollBar {
        parent: view
        ink: view.ink
        policy: view.horizontallyScrollable ? Controls.ScrollBar.AsNeeded : Controls.ScrollBar.AlwaysOff
        visible: view.horizontallyScrollable
        x: view.leftPadding
        y: view.height - height
        width: view.availableWidth
    }
}
