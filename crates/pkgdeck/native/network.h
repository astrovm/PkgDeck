#pragma once
#include <QQmlApplicationEngine>
#include <QNetworkAccessManager>
#include <QString>
#include <functional>

namespace pkgdeck {
void configure_network(QQmlApplicationEngine &engine);
QString download_metadata(const QString &url, const std::function<bool()> &cancelled);
QNetworkAccessManager *network_manager(const QString &category, QObject *parent);
}
