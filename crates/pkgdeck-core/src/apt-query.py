# Read-only host APT bridge for sandboxed builds. Uses the host's libapt ABI.
import json
import sys


def query(mode, needle, architecture):
    if mode not in ("detect", "search", "installed", "details"):
        raise ValueError("unknown APT query")
    try:
        import apt_pkg
        import apt.progress.base
    except ImportError as error:
        raise RuntimeError("Install python3-apt on the host to query APT from Flatpak") from error
    apt_pkg.init()
    apt_pkg.config.set("Dir::Cache::pkgcache", "")
    apt_pkg.config.set("Dir::Cache::srcpkgcache", "")
    if mode == "detect":
        return []
    cache = apt_pkg.Cache(apt.progress.base.OpProgress())
    policy = apt_pkg.DepCache(cache)
    records = apt_pkg.PackageRecords(cache)
    entries = []
    for package in cache.packages:
        installed = package.current_ver
        candidate = policy.get_candidate_ver(package)
        version = candidate or installed
        if version is None or (mode == "installed" and installed is None):
            continue
        if mode == "details" and (package.name != needle or version.arch != architecture):
            continue
        if version.file_list:
            entries.append((package, installed, candidate, version))
    entries.sort(key=lambda entry: (entry[3].file_list[0][0].id, entry[3].file_list[0][1]))
    result = []
    for package, installed, candidate, version in entries:
        records.lookup(version.file_list[0])
        if mode == "search" and needle.casefold() not in (package.name + " " + records.short_desc).casefold():
            continue
        phased = (policy.phasing_applied(package) if hasattr(policy, "phasing_applied")
                  else ("Phased-Update-Percentage" in records and records["Phased-Update-Percentage"] != "100"))
        upgradable = (installed is not None and candidate is not None
                      and apt_pkg.version_compare(candidate.ver_str, installed.ver_str) > 0
                      and package.selected_state != apt_pkg.SELSTATE_HOLD and not phased)
        result.append({"package": {
            "id": {"backend": "apt", "name": package.name, "architecture": version.arch, "scope": "system"},
            "display_name": package.name, "summary": records.short_desc,
            "installed_version": installed.ver_str if installed else None,
            "candidate_version": candidate.ver_str if candidate else None,
            "update": "available" if upgradable else ("current" if installed else "unknown")
        }, "description": records.long_desc, "homepage": records.homepage or None,
            "dependencies": [records["Depends"]] if "Depends" in records and records["Depends"] else []})
    return result


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4:
            raise ValueError("expected mode, query, architecture")
        print(json.dumps(query(*sys.argv[1:])))
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
