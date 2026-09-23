#pragma once
#include <QQmlApplicationEngine>
#include <QString>

namespace pkgdeck {
// Returns true when an existing window accepted this launch's input.
bool register_open_handler(QQmlApplicationEngine &engine, const QString &input);
}
