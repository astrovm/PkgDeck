import QtQuick
import QtQuick.Controls as Controls

// A ScrollView with the plain DeckScrollBar. A replaced ScrollView scrollbar
// is not laid out by the style, so it is placed along the right edge here.
Controls.ScrollView {
    id: view
    property color ink

    contentWidth: availableWidth
    clip: true
    // Keep content clear of the scrollbar while there is something to scroll.
    rightPadding: verticalBar.size < 1 ? verticalBar.width + 8 : 0
    Controls.ScrollBar.vertical: DeckScrollBar {
        id: verticalBar
        parent: view
        ink: view.ink
        x: view.mirrored ? 0 : view.width - width
        y: view.topPadding
        height: view.availableHeight
    }
    // Content always fits the width, so there is never a horizontal bar.
    Controls.ScrollBar.horizontal: DeckScrollBar {
        parent: view
        ink: view.ink
        visible: false
    }
}
