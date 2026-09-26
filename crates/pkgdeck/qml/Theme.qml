pragma Singleton
import QtQuick
import org.kde.kirigami as Kirigami

// Design tokens shared by every component: colours, radii, spacing, type
// scale and motion. The window binds appearance, reduceMotion and baseFont.
QtObject {
    id: theme

    // 0 follows the system, 1 is dark, 2 is light.
    property int appearance: 0
    property bool reduceMotion: false
    property font baseFont: Qt.application.font

    readonly property SystemPalette systemPalette: SystemPalette {}
    readonly property bool systemAppearance: appearance === 0
    // Some platforms leave the colour scheme hint Unknown; the palette then
    // decides, so status colours never stay light on a dark window.
    readonly property bool systemDark: Qt.styleHints.colorScheme === Qt.Dark
        || (Qt.styleHints.colorScheme !== Qt.Light && systemPalette.window.hslLightness < 0.5)
    readonly property bool dark: appearance === 1 || (systemAppearance && systemDark)

    readonly property color canvas: systemAppearance ? systemPalette.window : (dark ? "#0e1015" : "#f4f6f9")
    readonly property color surface: systemAppearance ? systemPalette.base : (dark ? "#161a21" : "#ffffff")
    readonly property color ink: systemAppearance ? systemPalette.text : (dark ? "#e9edf3" : "#18212d")
    readonly property color muted: systemAppearance ? systemPalette.placeholderText : (dark ? "#98a3b3" : "#5a6676")
    readonly property color accent: systemAppearance ? systemPalette.highlight : (dark ? "#7aa7ff" : "#2f68d8")
    // Derived tokens stay legible on any palette: tints of the text and
    // accent colours instead of palette roles that some themes leave flat.
    function tint(base, alpha) { return Qt.rgba(base.r, base.g, base.b, alpha); }
    readonly property color line: tint(ink, dark ? 0.12 : 0.13)
    readonly property color strongLine: tint(ink, dark ? 0.3 : 0.32)
    readonly property color hoverTint: tint(ink, dark ? 0.06 : 0.05)
    readonly property color selection: tint(accent, dark ? 0.2 : 0.14)
    // Dark text on the light dark-theme accent keeps primary buttons above
    // the 4.5:1 contrast minimum.
    readonly property color accentInk: systemAppearance ? systemPalette.highlightedText : (dark ? "#0e1015" : "#ffffff")
    readonly property color danger: dark ? "#f28b91" : "#c1343f"
    readonly property color success: dark ? "#6fd39a" : "#1b7a45"
    readonly property color warning: dark ? "#f2c56b" : "#9a6400"
    readonly property color heart: "#e34b5f"

    // Shape and spacing.
    readonly property int smallRadius: 5
    readonly property int controlRadius: 9
    readonly property int cardRadius: 12
    readonly property int spacingSmall: 6
    readonly property int spacing: 10
    readonly property int spacingLarge: 16
    readonly property int gutter: 16
    readonly property int scrollGutter: 16
    readonly property int controlHeight: Math.max(36, Math.round(baseFont.pointSize * 2.9))

    // Type scale, as multipliers of the base point size.
    readonly property real captionScale: 0.8
    readonly property real smallScale: 0.9
    readonly property real titleScale: 1.25
    readonly property real headingScale: 1.6
    function pointSize(scale) { return Math.max(1, baseFont.pointSize * scale); }

    // Motion. Every duration drops to zero when animations are off.
    readonly property bool motionEnabled: !reduceMotion && Kirigami.Units.shortDuration > 0
    readonly property int feedbackDuration: motionEnabled ? Math.round(Kirigami.Units.shortDuration * 0.8) : 0
    readonly property int revealDuration: motionEnabled ? Math.round(Kirigami.Units.shortDuration * 1.2) : 0
    readonly property int layoutDuration: motionEnabled ? Kirigami.Units.longDuration : 0
    readonly property int pulseDuration: motionEnabled ? Kirigami.Units.veryLongDuration * 2 : 0
}
