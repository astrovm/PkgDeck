#pragma once
#include <memory>
class PackageController;
// Construct the facade without loading a window, for native integration tests.
std::unique_ptr<PackageController> create_controller();
