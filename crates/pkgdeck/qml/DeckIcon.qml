import QtQuick
import QtQuick.Shapes

// Original vector artwork, embedded with the QML module. No icon theme or font.
// Drawn with Qt Quick Shapes so icons render on the GPU and stay sharp at any
// size; list rows can show many of them without software repaints.
Item {
    id: icon
    property string name: "package"
    property color ink: Theme.ink
    readonly property bool available: true
    implicitWidth: 20
    implicitHeight: 20
    Accessible.ignored: true // The adjacent control label carries the meaning.
    // Polylines in a 24×24 grid: [x0, y0, x1, y1, ...] per stroke.
    readonly property var drawings: ({
        "heart": [[12,21,3,12,2,8,3,4,7,3,12,7,17,3,21,4,22,8,21,12,12,21]],
        "package": [[3,7,12,2,21,7,21,17,12,22,3,17,3,7,12,12,21,7], [12,12,12,22], [7,4.8,16,9.8]],
        "discover": [[4,4,10,4,10,10,4,10,4,4], [14,4,20,4,20,10,14,10,14,4], [4,14,10,14,10,20,4,20,4,14], [14,14,20,14,20,20,14,20,14,14]],
        "search": [[15,15,21,21]],
        "installed": [[3,12,9,18,21,5]],
        "up": [[5,15,12,8,19,15]],
        "down": [[5,9,12,16,19,9]],
        "fwupd": [[6,6,18,6,18,18,6,18,6,6], [9,9,15,9,15,15,9,15,9,9], [3,9,6,9], [3,15,6,15], [18,9,21,9], [18,15,21,15], [9,3,9,6], [15,3,15,6], [9,18,9,21], [15,18,15,21]],
        "updates": [[12,21,12,3], [5,10,12,3,19,10]],
        "sources": [[3,3,21,3,21,10,3,10,3,3], [3,14,21,14,21,21,3,21,3,14], [7,6,7,7], [7,17,7,18]],
        "settings": [[3,6,8,6], [12,6,21,6], [3,12,14,12], [18,12,21,12], [3,18,6,18], [10,18,21,18], [8,3,8,9,12,9,12,3,8,3], [14,9,14,15,18,15,18,9,14,9], [6,15,6,21,10,21,10,15,6,15]],
        "help": [[9,8,9,6,15,6,16,9,12,12,12,14], [12,17,12,18]],
        "install": [[12,3,12,16], [7,11,12,16,17,11], [4,16,4,21,20,21,20,16]],
        "remove": [[4,6,20,6], [9,6,9,3,15,3,15,6], [6,6,7,21,17,21,18,6], [10,10,10,17], [14,10,14,17]],
        "refresh": [[20,3,20,9,14,9], [4,21,4,15,10,15]],
        "cancel": [[5,5,19,19], [19,5,5,19]],
        "activity": [[2,12,6,12,9,4,15,20,18,12,22,12]],
        "bell": [[6,17,6,10,7,7,9,5,12,4,15,5,17,7,18,10,18,17,20,19,4,19,6,17], [10,22,14,22]],
        "filter": [[3,5,21,5,14,13,14,20,10,22,10,13,3,5]],
        "apt": [[3,4,21,4,21,9,3,9,3,4], [5,9,5,21,19,21,19,9], [9,13,15,13]],
        "homebrew": [[4,6,16,6,16,21,4,21,4,6], [16,8,21,8,21,16,16,16], [7,2,7,3], [12,2,12,3]],
        "homebrew-cask": [[4,6,16,6,16,21,4,21,4,6], [16,8,21,8,21,16,16,16], [7,2,7,3], [12,2,12,3]],
        "docker": [[3,7,7,7,7,11,3,11,3,7], [8,7,12,7,12,11,8,11,8,7], [13,7,17,7,17,11,13,11,13,7], [8,2,12,2,12,6,8,6,8,2], [13,2,17,2,17,6,13,6,13,2], [2,12,20,12,19,17,15,20,8,20,4,17,2,12], [20,10,22,10,22,12,20,12]],
        "podman": [[12,2,18,5,21,11,19,18,12,22,5,18,3,11,6,5,12,2], [8,9,10,7,13,7,16,10,16,14,13,17,9,16,7,13,8,9], [11,11,13,11,13,13,11,13,11,11]],
        "cargo": [[3,7,12,2,21,7,21,17,12,22,3,17,3,7], [3,7,12,12,21,7], [12,12,12,22]],
        "npm": [[7,20,7,4]],
        "pnpm": [[7,20,7,4], [7,4,16,4,7,12]],
        "bun": [[8,20,8,4,15,4,15,10,8,10,16,10,16,17,8,17]],
        "pip": [[4,4,20,4,20,20,4,20,4,4], [9,8,15,8,15,13,12,16,9,13,9,8,9,8]],
        "pipx": [[4,4,20,4,20,20,4,20,4,4], [9,9,15,15], [15,9,9,15]],
        "uv": [[6,4,18,4,18,16,12,20,6,16,6,4]],
        "composer": [[12,3,20,8,12,13,4,8,12,3], [12,13,12,21]],
        "gem": [[6,8,12,3,18,8,15,21,9,21,6,8], [6,8,18,8], [9,8,12,13,15,8]],
        "warning": [[12,2,23,21,1,21,12,2], [12,8,12,14], [12,17,12,18]],
        "external": [[14,4,20,4,20,10], [20,4,11,13], [18,14,18,20,4,20,4,6,10,6]],
        "right": [[9,5,16,12,9,19]],
        "info": [[12,11,12,17], [12,7,12,8]]
    })
    // Circles and arcs that polylines cannot express, as SVG path data.
    readonly property var arcs: ({
        "search": "M 17 10 A 7 7 0 1 1 3 10 A 7 7 0 1 1 17 10",
        "info": "M 22 12 A 10 10 0 1 1 2 12 A 10 10 0 1 1 22 12",
        "help": "M 22 12 A 10 10 0 1 1 2 12 A 10 10 0 1 1 22 12",
        "refresh": "M 3.632 8.687 A 9 9 0 0 1 20.368 8.687 M 20.368 15.313 A 9 9 0 0 1 3.632 15.313"
    })
    readonly property string pathData: {
        const strokes = drawings[name] || drawings["package"];
        let data = strokes.map((points) => {
            let segment = "M " + points[0] + " " + points[1];
            for (let i = 2; i < points.length; i += 2)
                segment += " L " + points[i] + " " + points[i + 1];
            return segment;
        }).join(" ");
        if (arcs[name])
            data += " " + arcs[name];
        return data;
    }
    Shape {
        width: 24
        height: 24
        preferredRendererType: Shape.CurveRenderer
        transform: Scale { xScale: icon.width / 24; yScale: icon.height / 24 }
        ShapePath {
            strokeColor: icon.ink
            strokeWidth: 1.7
            fillColor: icon.name === "heart" ? icon.ink : "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { path: icon.pathData }
        }
    }
}
