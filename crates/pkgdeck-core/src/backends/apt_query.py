"""Read-only python-apt adapter. Invoked by the host Python, never the bundle."""
import json
import sys
import apt

mode, query = sys.argv[1:3]
if mode == 'detect':
    print('[]')
    sys.exit(0)
cache = apt.Cache()
result = []
for item in cache:
    version = item.candidate or item.installed
    if version is None:
        continue
    name = item.name.split(':')[0]
    architecture = version.architecture
    if mode == 'installed' and item.installed is None:
        continue
    if mode == 'search' and query.casefold() not in (name + ' ' + version.summary).casefold():
        continue
    if mode == 'details' and (name != query or architecture != sys.argv[3]):
        continue
    package = dict(
        id=dict(backend='apt', name=name, architecture=architecture, scope='system'),
        display_name=name, summary=version.summary,
        installed_version=item.installed.version if item.installed else None,
        candidate_version=item.candidate.version if item.candidate else None,
        update='available' if item.is_upgradable else ('current' if item.installed else 'unknown'),
    )
    result.append(dict(package=package, description=version.description,
                       homepage=version.homepage or None,
                       dependencies=[version.record['Depends']] if 'Depends' in version.record else []))
print(json.dumps(result))
