#include "network.h"
#include <QDateTime>
#include <QEventLoop>
#include <QNetworkReply>
#include <QTimer>
#include <QDir>
#include <QLockFile>
#include <QNetworkDiskCache>
#include <QNetworkRequest>
#include <QQmlNetworkAccessManagerFactory>
#include <QStandardPaths>
#include <memory>

namespace {
// Each cache has a process lock. A second window/process continues without
// disk caching instead of sharing Qt's non-concurrent on-disk format.
class DiskCache final : public QNetworkDiskCache {
    std::unique_ptr<QLockFile> lock;
public:
    DiskCache(const QString &category, QObject *parent) : QNetworkDiskCache(parent) {
        const auto root = QStandardPaths::writableLocation(QStandardPaths::CacheLocation);
        if (root.isEmpty()) return;
        const auto directory = root + "/" + category;
        if (!QDir().mkpath(directory)) return;
        lock = std::make_unique<QLockFile>(directory + "/cache.lock");
        if (!lock->tryLock(0)) return;
        setCacheDirectory(directory);
        setMaximumCacheSize(category == "screenshots" ? 64 * 1024 * 1024 : 8 * 1024 * 1024);
    }
    QIODevice *prepare(const QNetworkCacheMetaData &original) override {
        if (cacheDirectory().isEmpty() || !original.saveToDisk()) return nullptr;
        auto metadata = original;
        const auto now = QDateTime::currentDateTimeUtc();
        // Honor shorter server lifetimes, including immediate revalidation.
        const auto limit = now.addDays(7);
        if (!metadata.expirationDate().isValid()) metadata.setExpirationDate(now.addDays(1));
        else if (metadata.expirationDate() > limit) metadata.setExpirationDate(limit);
        return QNetworkDiskCache::prepare(metadata);
    }
    QNetworkCacheMetaData metaData(const QUrl &url) override {
        auto metadata = QNetworkDiskCache::metaData(url);
        if (metadata.isValid() && metadata.expirationDate() <= QDateTime::currentDateTimeUtc()) {
            remove(url);
            return {};
        }
        return metadata;
    }
};
class Manager final : public QNetworkAccessManager {
public:
    using QNetworkAccessManager::QNetworkAccessManager;
protected:
    QNetworkReply *createRequest(Operation operation, const QNetworkRequest &original, QIODevice *data) override {
        auto request = original;
        request.setAttribute(QNetworkRequest::CacheLoadControlAttribute, QNetworkRequest::PreferCache);
        request.setAttribute(QNetworkRequest::RedirectPolicyAttribute, QNetworkRequest::NoLessSafeRedirectPolicy);
        request.setTransferTimeout(15000);
        return QNetworkAccessManager::createRequest(operation, request, data);
    }
};
class Factory final : public QQmlNetworkAccessManagerFactory {
public:
    QNetworkAccessManager *create(QObject *parent) override {
        return pkgdeck::network_manager("screenshots", parent);
    }
};
}
namespace pkgdeck {
QNetworkAccessManager *network_manager(const QString &category, QObject *parent) {
    auto *manager = new Manager(parent);
    auto *cache = new DiskCache(category, manager);
    if (cache->cacheDirectory().isEmpty()) delete cache;
    else manager->setCache(cache);
    return manager;
}
void configure_network(QQmlApplicationEngine &engine) {
    // QQmlEngine does not take ownership; the stateless factory outlives engines.
    static Factory factory;
    engine.setNetworkAccessManagerFactory(&factory);
}
QString download_metadata(const QString &address, const std::function<bool()> &cancelled) {
    const QUrl url(address);
    if (cancelled() || url.scheme() != "https") return {};
    QByteArray bytes;
    bool oversized = false;
    std::unique_ptr<QNetworkAccessManager> manager(network_manager("metadata", nullptr));
    QNetworkRequest request(url);
    request.setRawHeader("Accept", "application/json");
    if (url.host() == "api.snapcraft.io") request.setRawHeader("Snap-Device-Series", "16");
    auto *reply = manager->get(request);
    QEventLoop loop;
    QTimer deadline, cancellation;
    deadline.setSingleShot(true);
    QObject::connect(&deadline, &QTimer::timeout, &loop, [&] { reply->abort(); loop.quit(); });
    QObject::connect(&cancellation, &QTimer::timeout, &loop, [&] { if (cancelled()) { reply->abort(); loop.quit(); } });
    QObject::connect(reply, &QIODevice::readyRead, reply, [&] {
        bytes += reply->readAll();
        if (bytes.size() > 2 * 1024 * 1024) { oversized = true; reply->abort(); }
    });
    QObject::connect(reply, &QNetworkReply::finished, &loop, &QEventLoop::quit);
    deadline.start(4000);
    cancellation.start(50);
    if (!reply->isFinished()) loop.exec();
    if (reply->isOpen()) bytes += reply->readAll();
    if (cancelled() || oversized || bytes.size() > 2 * 1024 * 1024 || reply->error() != QNetworkReply::NoError) return {};
    return QString::fromUtf8(bytes);
}
}
