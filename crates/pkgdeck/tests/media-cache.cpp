// Synthetic HTTPS fixture exercising the same QML network factory as PkgDeck.
#include "../native/network.h"
#include <QBuffer>
#include <QFile>
#include <QGuiApplication>
#include <QImage>
#include <QNetworkDiskCache>
#include <QQmlComponent>
#include <QSslCertificate>
#include <QSslConfiguration>
#include <QSslKey>
#include <QSslSocket>
#include <QTcpServer>
#include <QTimer>
#include <QEventLoop>
#include <QElapsedTimer>
#include <QTextStream>
#include <memory>
#include <cstdio>

static QByteArray read(const QString &path) {
    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) return {};
    return file.readAll();
}
class Server : public QTcpServer {
public:
    QSslCertificate certificate;
    QSslKey key;
    QByteArray image;
    int requests = 0;
    explicit Server(const QString &directory) {
        certificate = QSslCertificate(read(directory + "/cert.pem"));
        key = QSslKey(read(directory + "/key.pem"), QSsl::Rsa);
        QImage pixel(16, 16, QImage::Format_RGB32);
        pixel.fill(Qt::blue);
        QBuffer buffer(&image);
        buffer.open(QIODevice::WriteOnly);
        if (!pixel.save(&buffer, "WEBP")) qFatal("WebP encoder plugin missing");
    }
protected:
    void incomingConnection(qintptr descriptor) override {
        auto *socket = new QSslSocket(this);
        socket->setSocketDescriptor(descriptor);
        socket->setLocalCertificate(certificate);
        socket->setPrivateKey(key);
        connect(socket, &QSslSocket::disconnected, socket, &QObject::deleteLater);
        connect(socket, &QSslSocket::readyRead, socket, [this, socket, request = QByteArray()]() mutable {
            request += socket->readAll();
            if (!request.contains("\r\n\r\n")) return;
            ++requests;
            if (request.startsWith("GET /slow")) return;
            const auto policy = request.startsWith("GET /expired") ? "max-age=0" : request.startsWith("GET /private") ? "no-store" : "max-age=3600";
            const auto body = request.startsWith("GET /large") ? QByteArray(3 * 1024 * 1024, 'x') : request.startsWith("GET /metadata") ? QByteArray("{\"name\":\"Synthetic Player\"}") : image;
            socket->write(QByteArray("HTTP/1.1 200 OK\r\nContent-Type: image/webp\r\nCache-Control: ") + policy
                + "\r\nContent-Length: " + QByteArray::number(body.size()) + "\r\nConnection: close\r\n\r\n" + body);
            socket->disconnectFromHost();
        });
        socket->startServerEncryption();
    }
};
static bool imageLoads(const QString &url) {
    QQmlApplicationEngine engine;
    pkgdeck::configure_network(engine);
    QQmlComponent component(&engine);
    component.setData(("import QtQuick\nImage { width: 16; height: 16; cache: false; asynchronous: true; source: \"" + url + "\" }").toUtf8(), QUrl());
    std::unique_ptr<QObject> image(component.create());
    if (!image) { qWarning() << component.errors(); return false; }
    QEventLoop loop;
    QTimer poll, timeout;
    timeout.setSingleShot(true);
    QObject::connect(&poll, &QTimer::timeout, &loop, [&] {
        const auto status = image->property("status").toInt();
        if (status == 1 || status == 3) loop.quit();
    });
    QObject::connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
    poll.start(10); timeout.start(10000); loop.exec();
    return image->property("status").toInt() == 1;
}
int main(int argc, char **argv) {
    QGuiApplication app(argc, argv);
    app.setApplicationName("PkgDeck-MediaTest");
    if (argc != 3 || !QSslSocket::supportsSsl()) return 1;
    const QString directory = QString::fromLocal8Bit(argv[1]);
    const bool reuse = QByteArray(argv[2]) == "reuse";
    Server server(directory);
    auto configuration = QSslConfiguration::defaultConfiguration();
    configuration.addCaCertificate(server.certificate);
    QSslConfiguration::setDefaultConfiguration(configuration);
    const auto port = reuse ? read(directory + "/port").trimmed().toUShort() : 0;
    if (!server.listen(QHostAddress::LocalHost, port)) return 2;
    if (!reuse) {
        QFile file(directory + "/port");
        if (!file.open(QIODevice::WriteOnly)) return 3;
        file.write(QByteArray::number(server.serverPort()));
    }
    const auto base = QString("https://localhost:%1").arg(server.serverPort());
    if (!imageLoads(base + "/cached.webp")) return 4;
    if (server.requests != (reuse ? 0 : 1)) { qWarning() << "unexpected requests after restart:" << server.requests; return 5; }
    const int before = server.requests;
    if (!imageLoads(base + "/expired.webp") || !imageLoads(base + "/expired.webp")) return 6;
    if (server.requests != before + 2) { qWarning() << "expired response was reused"; return 7; }
    const int privateBefore = server.requests;
    if (!imageLoads(base + "/private.webp") || !imageLoads(base + "/private.webp") || server.requests != privateBefore + 2) return 9;
    if (pkgdeck::download_metadata(base + "/metadata", [] { return false; }) != "{\"name\":\"Synthetic Player\"}") return 10;
    if (!pkgdeck::download_metadata(base + "/large", [] { return false; }).isEmpty()) return 11;
    QElapsedTimer elapsed;
    elapsed.start();
    if (!pkgdeck::download_metadata(base + "/slow", [&] { return elapsed.elapsed() > 100; }).isEmpty() || elapsed.elapsed() > 1500) return 12;
    const int cancelledBefore = server.requests;
    if (!pkgdeck::download_metadata(base + "/cancelled", [] { return true; }).isEmpty() || server.requests != cancelledBefore) return 13;
    // A trusted certificate with the wrong hostname must still be rejected.
    if (imageLoads(QString("https://127.0.0.1:%1/rejected.webp").arg(server.serverPort()))) return 8;
    std::puts(reuse ? "PASS persistent cache, expiry, WebP and certificate validation" : "PASS HTTPS screenshot loading, expiry and WebP");
}
