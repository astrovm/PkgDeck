#include "opening.h"
#include <QLocalServer>
#include <QLocalSocket>
#include <QMetaObject>
#include <QStandardPaths>
#include <QStringList>
#include <QTimer>
#include <QUrl>
#include <QVariant>
#include <QWindow>
#include <memory>
#include <unistd.h>

namespace pkgdeck {
namespace {
void deliver_pending(QQmlApplicationEngine &engine, QStringList &pending) {
    if (engine.rootObjects().isEmpty()) return;
    auto *root = engine.rootObjects().first();
    for (const auto &input : pending) {
        if (!input.isEmpty())
            QMetaObject::invokeMethod(root, "openExternalInput", Q_ARG(QVariant, QVariant(input)));
    }
    if (!pending.isEmpty()) {
        if (auto *window = qobject_cast<QWindow *>(root)) {
            if (window->windowState() == Qt::WindowMinimized) window->showNormal();
            else window->show();
            window->raise();
            window->requestActivate();
        }
    }
    pending.clear();
}
}

bool register_open_handler(QQmlApplicationEngine &engine, const QString &input) {
    const auto runtime = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    if (runtime.isEmpty()) return false;
    const auto name = runtime + "/pkgdeck-open-" + QString::number(geteuid());
    const auto forward = [&] {
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
        return false;
    };
    if (forward()) return true;
    auto *server = new QLocalServer(&engine);
    server->setSocketOptions(QLocalServer::UserAccessOption);
    if (!server->listen(name)) {
        if (forward()) {
            delete server;
            return true;
        }
        QLocalSocket probe;
        probe.connectToServer(name, QIODevice::WriteOnly);
        if (!probe.waitForConnected(200)
            && (probe.error() == QLocalSocket::ConnectionRefusedError
                || probe.error() == QLocalSocket::ServerNotFoundError)) {
            QLocalServer::removeServer(name);
            server->listen(name);
        }
        if (!server->isListening()) {
            delete server;
            return false;
        }
    }
    auto queuedInputs = std::make_shared<QStringList>();
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreated, server,
                     [server, &engine, queuedInputs](QObject *root, const QUrl &) {
        if (root) {
            QTimer::singleShot(0, server, [&engine, queuedInputs] { deliver_pending(engine, *queuedInputs); });
        }
    });
    QObject::connect(server, &QLocalServer::newConnection, server, [server, &engine, queuedInputs] {
        while (auto *peer = server->nextPendingConnection()) {
            auto bytes = std::make_shared<QByteArray>();
            auto handled = std::make_shared<bool>(false);
            auto consume = [peer, bytes, handled, &engine, server, queuedInputs] {
                if (*handled) return;
                bytes->append(peer->readAll());
                const auto end = bytes->indexOf('\0');
                if (bytes->size() > 8192 || end < 0) {
                    if (bytes->size() > 8192) peer->disconnectFromServer();
                    return;
                }
                *handled = true;
                const auto input = QString::fromUtf8(bytes->constData(), end);
                queuedInputs->append(input);
                QTimer::singleShot(0, server, [&engine, queuedInputs] { deliver_pending(engine, *queuedInputs); });
                peer->disconnectFromServer();
            };
            QObject::connect(peer, &QLocalSocket::readyRead, server, consume);
            consume();
            QObject::connect(peer, &QLocalSocket::disconnected, peer, &QObject::deleteLater);
        }
    });
    return false;
}
}
