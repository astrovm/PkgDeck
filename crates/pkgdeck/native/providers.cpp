#include "providers.h"
#include "network.h"
#include "pkgdeck/src/network.cxx.h"
namespace pkgdeck {
QString fetch_metadata(const QString &address, const LookupCancellation &cancel) {
    return download_metadata(address, [&cancel] { return cancel.cancelled(); });
}
}
