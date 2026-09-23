#include "network.h"

#include <QApplication>
#include <QCoreApplication>
#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QUrl>

extern "C" int pkgdeck_run_gui(int argc, char **argv, const char *version) {
    QApplication app(argc, argv);
    QCoreApplication::setApplicationName(QStringLiteral("PkgDeck"));
    QCoreApplication::setApplicationVersion(QString::fromUtf8(version));
    QCoreApplication::setOrganizationName(QStringLiteral("astrovm"));
    QCoreApplication::setOrganizationDomain(QStringLiteral("github.com/astrovm"));
    QGuiApplication::setDesktopFileName(QStringLiteral("io.github.astrovm.PkgDeck"));

    QQmlApplicationEngine engine;
    pkgdeck::configure_network(engine);
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreationFailed,
                     &app, [] { QCoreApplication::exit(1); }, Qt::QueuedConnection);
    engine.load(QUrl(QStringLiteral("qrc:/qt/qml/io/github/astrovm/PkgDeck/qml/Main.qml")));
    return app.exec();
}
