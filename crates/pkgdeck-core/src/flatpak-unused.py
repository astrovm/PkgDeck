# Read-only helper embedded in the executable, run with isolated system Python.
import json
import sys

try:
    import gi
    gi.require_version("Flatpak", "1.0")
    from gi.repository import Flatpak
except (ImportError, ValueError):
    sys.exit(78)

rows = []
for scope, constructor in (("user", Flatpak.Installation.new_user),
                           ("system", Flatpak.Installation.new_system)):
    installation = constructor(None)
    for ref in installation.list_unused_refs(None, None):
        rows.append({"scope": scope, "reference": ref.format_ref(),
                     "bytes": ref.get_installed_size()})
print(json.dumps(rows))
