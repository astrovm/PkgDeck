#include "network.h"
#include "opening.h"

#include <QApplication>
#include <QCoreApplication>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QUrl>

#include <cstdio>

namespace {
// Qt unloads its plugins from a static destructor once the process exits, and
// logs each unload when plugin debugging is on. On macOS the default handler
// sends that through Apple's unified logging, whose Qt-side static state has
// already been destroyed by then, and the stale free traps (SIGTRAP). After the
// event loop, write messages straight to stderr, which needs no such state.
void shutdownMessageHandler(QtMsgType, const QMessageLogContext &context, const QString &message) {
    QByteArray line;
    if (context.category && qstrcmp(context.category, "default") != 0)
        line.append(context.category).append(": ");
    line.append(message.toLocal8Bit()).append('\n');
    std::fwrite(line.constData(), 1, size_t(line.size()), stderr);
}
}

extern "C" int pkgdeck_run_gui(int argc, char **argv, const char *version) {
    QApplication app(argc, argv);
    QCoreApplication::setApplicationName(QStringLiteral("PkgDeck"));
    QCoreApplication::setApplicationVersion(QString::fromUtf8(version));
    QCoreApplication::setOrganizationName(QStringLiteral("astrovm"));
    QCoreApplication::setOrganizationDomain(QStringLiteral("github.com/astrovm"));
    QGuiApplication::setDesktopFileName(QStringLiteral("io.github.astrovm.PkgDeck"));

    QQmlApplicationEngine engine;
    pkgdeck::configure_network(engine);
    QString opening;
    bool smokeTest = false;
    for (int index = 1; index < argc; ++index) {
        const auto argument = QString::fromLocal8Bit(argv[index]);
        if (argument == QStringLiteral("--smoke-test")) smokeTest = true;
        if (opening.isEmpty()
            && (argument.startsWith(QLatin1Char('/'))
                || argument.startsWith(QStringLiteral("file://"))
                || argument.startsWith(QStringLiteral("https://"))
                || argument.startsWith(QStringLiteral("flatpak+https://")))) {
            opening = argument;
        }
    }
    if (!smokeTest && pkgdeck::register_open_handler(engine, opening)) {
        qInstallMessageHandler(shutdownMessageHandler);
        return 0;
    }
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed,
                     &app, [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.load(QUrl(QStringLiteral("qrc:/qt/qml/io/github/astrovm/PkgDeck/qml/Main.qml")));
    const int status = app.exec();
    qInstallMessageHandler(shutdownMessageHandler);
    return status;
}
