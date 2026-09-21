#include "controller.h"
#include "pkgdeck/src/controller.cxxqt.h"
std::unique_ptr<PackageController> create_controller() {
    return std::make_unique<PackageController>();
}
