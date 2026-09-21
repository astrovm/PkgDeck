# Distribution

## Homebrew

The repository doubles as a third-party Homebrew tap. Its formula lives in
[`Formula/pkgdeck.rb`](../Formula/pkgdeck.rb).

| Host | Installed commands | GUI |
| --- | --- | --- |
| macOS | `pkd`, `pkgdeck` | `PkgDeck.app` |
| Linux | `pkd` | Not installed by Homebrew |

The initial recipe builds from main and has no stable release stanza. After the
recipe is merged, install it with:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install --HEAD astrovm/pkgdeck/pkgdeck
```

Use the explicit repository URL: this project is not named `homebrew-pkgdeck`.
This is our tap, not a submission to Homebrew core. To update a HEAD installation:

```sh
brew upgrade --fetch-HEAD astrovm/pkgdeck/pkgdeck
```

On macOS, Qt is supplied by Homebrew. The formula builds a checksum-pinned private
Kirigami runtime and installs a launcher that selects its QML imports and the
Basic Qt Controls style. Launch from a terminal with `pkgdeck`, or make it
available in Finder:

```sh
mkdir -p ~/Applications
ln -s "$(brew --prefix astrovm/pkgdeck/pkgdeck)/PkgDeck.app" ~/Applications/PkgDeck.app
```

On Linux, the formula builds only the Qt-free CLI. APT reads need the optional
`pkgdeck-apt-query` companion. If the distribution's `libapt-pkg-dev` headers are
present at build time, the formula installs the reader beside `pkd`. Without it,
APT is unavailable; other detected managers remain usable. The formula does not
install distribution development packages or elevate Homebrew.

The Homebrew workflow builds and smoke-tests the recipe on Linux and macOS in
isolated CI runners. macOS validation must pass before treating that package as
supported; a successful Linux build does not establish macOS compatibility.

### Publishing a stable version

1. Run the project checks and Homebrew workflow. Review the macOS GUI smoke result
   and the Linux check that no GUI command is installed.
2. Set the workspace version, tag the reviewed commit, and follow the existing
   Linux packaging workflow. Do not reuse or move a published tag.
3. Download the tagged source archive from
   `https://github.com/astrovm/PkgDeck/archive/refs/tags/vVERSION.tar.gz` and compute
   its SHA-256. Add its actual `url` and `sha256` below `homepage` in the formula,
   keeping `head` for development installations. Submit this change through a PR.
4. Test the stable formula on both platforms before merging. After users run
   `brew update`, `brew install astrovm/pkgdeck/pkgdeck` installs the stable version.

No release archive, checksum, bottle, signing identity, or notarization result is
assumed by the initial recipe. The `.app` uses Homebrew dependencies and is not a
standalone downloadable macOS application.

## Linux packages

The existing scripts build native packages, AppImage, Flatpak, and Snap artifacts.
Use the same verified build environment as CI:

```sh
scripts/verify.sh full --engine podman
```

See [`scripts/package.sh`](../scripts/package.sh),
[`scripts/bundle.sh`](../scripts/bundle.sh), and the packaging jobs in
[CI](../.github/workflows/ci.yml) for the supported invocations and artifact checks.

Native packages and AppImage can manage host packages. Flatpak and strict Snap
builds currently fail closed for host operations. They remain experimental until
installed-package tests validate a host bridge; network access for icons and
screenshots does not enable package-manager access.

The APT reader and bundled libraries keep their original licenses. Package
scripts include dependency notices alongside PkgDeck's MIT license.
