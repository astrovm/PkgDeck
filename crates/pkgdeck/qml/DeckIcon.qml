import QtQuick

// Original vector artwork, embedded with the QML module. No icon theme or font.
Canvas {
    id: icon
    property string name: "package"
    property color ink: "#ffffff"
    implicitWidth: 20
    implicitHeight: 20
    Accessible.ignored: true // The adjacent control label carries the meaning.
    readonly property var drawings: ({
        "package": [[3,7,12,2,21,7,21,17,12,22,3,17,3,7,12,12,21,7], [12,12,12,22], [7,4.8,16,9.8]],
        "discover": [[4,4,10,4,10,10,4,10,4,4], [14,4,20,4,20,10,14,10,14,4], [4,14,10,14,10,20,4,20,4,14], [14,14,20,14,20,20,14,20,14,14]],
        "search": [[15,15,21,21]],
        "installed": [[3,12,9,18,21,5]],
        "updates": [[12,21,12,3], [5,10,12,3,19,10]],
        "sources": [[3,3,21,3,21,10,3,10,3,3], [3,14,21,14,21,21,3,21,3,14], [7,6,7,7], [7,17,7,18]],
        "settings": [[3,6,8,6], [12,6,21,6], [3,12,14,12], [18,12,21,12], [3,18,6,18], [10,18,21,18], [8,3,8,9,12,9,12,3,8,3], [14,9,14,15,18,15,18,9,14,9], [6,15,6,21,10,21,10,15,6,15]],
        "help": [[9,8,9,6,15,6,16,9,12,12,12,14], [12,17,12,18]],
        "install": [[12,3,12,16], [7,11,12,16,17,11], [4,16,4,21,20,21,20,16]],
        "remove": [[4,6,20,6], [9,6,9,3,15,3,15,6], [6,6,7,21,17,21,18,6], [10,10,10,17], [14,10,14,17]],
        "refresh": [[20,3,20,9,14,9], [4,21,4,15,10,15]],
        "cancel": [[5,5,19,19], [19,5,5,19]],
        "apt": [[3,4,21,4,21,9,3,9,3,4], [5,9,5,21,19,21,19,9], [9,13,15,13]],
        "homebrew": [[4,6,16,6,16,21,4,21,4,6], [16,8,21,8,21,16,16,16], [7,2,7,3], [12,2,12,3]],
        "warning": [[12,2,23,21,1,21,12,2], [12,8,12,14], [12,17,12,18]]
    })
    onNameChanged: requestPaint()
    onInkChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
    onPaint: {
        const ctx = getContext("2d");
        ctx.reset();
        ctx.scale(width / 24, height / 24);
        ctx.strokeStyle = ink;
        ctx.lineWidth = 1.7;
        ctx.lineCap = "round";
        ctx.lineJoin = "round";
        const paths = drawings[name] || drawings.package;
        for (const points of paths) {
            ctx.beginPath();
            ctx.moveTo(points[0], points[1]);
            for (let i = 2; i < points.length; i += 2)
                ctx.lineTo(points[i], points[i + 1]);
            ctx.stroke();
        }
        if (name === "search" || name === "help" || name === "refresh") {
            ctx.beginPath();
            if (name === "search")
                ctx.arc(10, 10, 7, 0, Math.PI * 2);
            else if (name === "help")
                ctx.arc(12, 12, 10, 0, Math.PI * 2);
            else {
                ctx.arc(12, 12, 9, Math.PI * 1.12, Math.PI * 1.88);
                ctx.moveTo(20.37, 15.31);
                ctx.arc(12, 12, 9, Math.PI * 0.12, Math.PI * 0.88);
            }
            ctx.stroke();
        }
    }
}
