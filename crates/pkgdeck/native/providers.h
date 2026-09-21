#pragma once
#include <QString>
namespace pkgdeck {
struct LookupCancellation;
QString fetch_metadata(const QString &url, const LookupCancellation &cancel);
}
