#include "opening.h"
#include <QLocalServer>
#include <QLocalSocket>
#include <QMetaObject>
#include <QStandardPaths>
#include <QVariant>
#include <QWindow>
#include <memory>
#include <unistd.h>

namespace pkgdeck {
bool register_open_handler(QQmlApplicationEngine &engine, const QString &input) {
    const auto runtime = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    if (runtime.isEmpty()) return false;
    const auto name = runtime + "/pkgdeck-open-" + QString::number(geteuid());
    if (!input.isEmpty()) {
        QLocalSocket existing;
        existing.connectToServer(name, QIODevice::WriteOnly);
        if (existing.waitForConnected(200)) {
            auto bytes = input.toUtf8();
            if (bytes.size() > 8192) return false;
            bytes.append('\0');
            existing.write(bytes);
            if (!existing.waitForBytesWritten(1000)) return false;
            existing.disconnectFromServer();
            return true;
        }
    }
    auto *server = new QLocalServer(&engine);
    server->setSocketOptions(QLocalServer::UserAccessOption);
    if (!server->listen(name)) {
        delete server;
        return false;
    }
    QObject::connect(server, &QLocalServer::newConnection, server, [server, &engine] {
        while (auto *peer = server->nextPendingConnection()) {
            auto pending = std::make_shared<QByteArray>();
            QObject::connect(peer, &QLocalSocket::readyRead, server, [peer, pending, &engine] {
                pending->append(peer->readAll());
                const auto end = pending->indexOf('\0');
                if (pending->size() > 8192 || end < 0) {
                    if (pending->size() > 8192) peer->disconnectFromServer();
                    return;
                }
                const auto input = QString::fromUtf8(pending->constData(), end);
                if (!engine.rootObjects().isEmpty()) {
                    auto *root = engine.rootObjects().first();
                    QMetaObject::invokeMethod(root, "openExternalInput", Q_ARG(QVariant, QVariant(input)));
                    if (auto *window = qobject_cast<QWindow *>(root)) {
                        window->raise();
                        window->requestActivate();
                    }
                }
                peer->disconnectFromServer();
            });
            QObject::connect(peer, &QLocalSocket::disconnected, peer, &QObject::deleteLater);
        }
    });
    return false;
}
}
