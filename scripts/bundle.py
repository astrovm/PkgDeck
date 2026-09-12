#!/usr/bin/env python3
"""Stage both frontends, pinned Qt/Kirigami, and their ELF dependencies."""
import os
from pathlib import Path
import re
import shutil
import subprocess

root = Path(__file__).resolve().parents[1]
out = root / "build/AppDir"
out.mkdir(parents=True, exist_ok=True)
qt = Path(os.environ["QT_ROOT_DIR"])
kde = Path(os.environ["PKGDECK_SDK_PREFIX"])
lib = out / "usr/lib"
lib.mkdir(parents=True, exist_ok=True)
for name in ("pkd", "pkgdeck"):
    dest = out / "usr/bin" / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / "target/release" / name, dest)
for prefix in (qt, kde):
    for directory in ("qml", "plugins"):
        source = prefix / directory
        if source.exists():
            shutil.copytree(source, out / "usr" / directory, dirs_exist_ok=True)
    for source in (prefix / "lib").glob("*.so*"):
        shutil.copy2(source, lib / source.name)
shutil.copy2(root / "packaging/appimage/AppRun", out / "AppRun")
(out / "AppRun").chmod(0o755)
for suffix, directory in (("desktop", "applications"), ("metainfo.xml", "metainfo"), ("svg", "icons/hicolor/scalable/apps")):
    name = f"io.github.astrovm.PkgDeck.{suffix}"
    dest = out / "usr/share" / directory
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copy2(root / "assets" / name, dest / name)
    if suffix in ("desktop", "svg"):
        shutil.copy2(root / "assets" / name, out / name)
(out / "usr/bin/qt.conf").write_text("[Paths]\nPrefix=..\nLibraries=lib\nPlugins=plugins\nQmlImports=qml\n")
# The target supplies glibc and the graphics driver. Ship the remaining dependency
# closure, including dependencies of QML and platform plugins loaded at runtime.
system = re.compile(r"^(ld-linux.*|lib(c|m|dl|pthread|rt|resolv|util|anl)\.so\..*|lib(GL|EGL|GLX|GLdispatch|OpenGL)\.so\..*)$")
queue = [p for p in out.rglob("*") if p.is_file() and (".so" in p.name or p.name in ("pkd", "pkgdeck"))]
seen = set()
env = dict(os.environ, LD_LIBRARY_PATH=f"{lib}:{qt / 'lib'}:{kde / 'lib'}")
while queue:
    path = queue.pop()
    if path in seen:
        continue
    seen.add(path)
    result = subprocess.run(["ldd", str(path)], env=env, capture_output=True, text=True)
    if "not found" in result.stdout:
        raise SystemExit(f"Unresolved runtime dependency in {path}:\n{result.stdout}")
    if result.returncode:
        if path.read_bytes()[:4] == b"\x7fELF":
            raise SystemExit(result.stderr)
        continue
    for name, source in re.findall(r"^\s*(\S+) => (/\S+)", result.stdout, re.MULTILINE):
        target = lib / name
        if not system.match(name) and not target.exists():
            shutil.copy2(source, target)
            queue.append(target)
licenses = out / "usr/share/licenses/pkgdeck"
licenses.mkdir(parents=True, exist_ok=True)
shutil.copy2(root / "LICENSE", licenses / "LICENSE")
