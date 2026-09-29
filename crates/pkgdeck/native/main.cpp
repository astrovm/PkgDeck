#include "network.h"
#include "opening.h"
#ifdef Q_OS_MACOS
#include "macos.h"
#endif

#include <QApplication>
#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include <QSettings>
#include <QStandardPaths>
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

// Earlier releases stored settings and caches under the "astrovm"
// organization (~/.config/astrovm, ~/.cache/astrovm) instead of the pkgdeck
// folders the rest of PkgDeck uses. Carry the settings over once and drop the
// old cache, which is rebuilt on demand.
void migrateLegacyLocations() {
#ifdef Q_OS_DARWIN
    const auto legacyOrganization = QStringLiteral("github.com/astrovm");
#else
    const auto legacyOrganization = QStringLiteral("astrovm");
#endif
    QSettings current;
    QSettings legacy(legacyOrganization, QCoreApplication::applicationName());
    const auto keys = legacy.allKeys();
    if (!keys.isEmpty() && current.allKeys().isEmpty()) {
        for (const auto &key : keys) current.setValue(key, legacy.value(key));
        current.sync();
    }
    if (!keys.isEmpty() && current.status() == QSettings::NoError) {
        legacy.clear();
        legacy.sync();
#ifndef Q_OS_DARWIN
        const QFileInfo file(legacy.fileName());
        QFile::remove(file.filePath());
        QDir().rmdir(file.path());
#endif
    }
    const auto cache = QStandardPaths::writableLocation(QStandardPaths::GenericCacheLocation);
    if (!cache.isEmpty()) {
        QDir(cache + QStringLiteral("/astrovm/PkgDeck")).removeRecursively();
        QDir().rmdir(cache + QStringLiteral("/astrovm"));
    }
}
}

extern "C" int pkgdeck_run_gui(int argc, char **argv, const char *version) {
    QApplication app(argc, argv);
    QCoreApplication::setApplicationName(QStringLiteral("PkgDeck"));
    QCoreApplication::setApplicationVersion(QString::fromUtf8(version));
    // Linux keeps settings in ~/.config/pkgdeck and caches in ~/.cache/pkgdeck,
    // next to the core's own files. macOS names preferences after the domain,
    // which reverses to the bundle identifier io.github.astrovm.PkgDeck.
    QCoreApplication::setOrganizationName(QStringLiteral("pkgdeck"));
    QCoreApplication::setOrganizationDomain(QStringLiteral("astrovm.github.io"));
    QGuiApplication::setDesktopFileName(QStringLiteral("io.github.astrovm.PkgDeck"));
    migrateLegacyLocations();

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
#ifdef Q_OS_MACOS
    engine.rootContext()->setContextProperty(QStringLiteral("macNative"), new pkgdeck::MacNative(&app));
#endif
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed,
                     &app, [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.load(QUrl(QStringLiteral("qrc:/qt/qml/io/github/astrovm/PkgDeck/qml/Main.qml")));
    const int status = app.exec();
    qInstallMessageHandler(shutdownMessageHandler);
    return status;
}
