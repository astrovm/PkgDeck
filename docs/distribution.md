# Distribution

## Homebrew

The repository doubles as a third-party Homebrew tap. Its formula lives in
[`Formula/pkgdeck.rb`](../Formula/pkgdeck.rb).

| Host | Installed commands | GUI |
| --- | --- | --- |
| macOS | `pkd`, `pkgdeck` | `PkgDeck.app` |
| Linux | `pkd` | Not installed by Homebrew |

Before the first tagged release, install the development recipe with:

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

The CI workflow builds and smoke-tests the Homebrew recipe on Linux and macOS in
isolated runners. macOS validation must pass before treating that package as
supported; a successful Linux build does not establish macOS compatibility.

### Publishing a stable version

1. Run the project CI checks. Review the macOS Homebrew GUI smoke result
   and the Linux check that no GUI command is installed.
2. Set the workspace version, tag the reviewed commit, and follow the existing
   Linux packaging workflow. Do not reuse or move a published tag.
3. Publishing the GitHub release triggers `Publish / Homebrew`, which opens a PR
   with the immutable archive URL and SHA-256 while retaining `head` for development.
4. Let CI test that PR with Homebrew on both platforms before merging. After users run
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

Tags matching `v*` build both Linux architectures, run the required checks, create
a GitHub release with AppImage update metadata, Flatpak bundles, Snap packages,
checksums, and provenance, then upload Snap packages when
`SNAPCRAFT_STORE_CREDENTIALS` is configured. Snap uses classic confinement and
requires Snap Store approval. `Publish Flatpak repository` dispatches the immutable
release bundles to [`astrovm/flatpak`](https://github.com/astrovm/flatpak) when
`FLATPAK_REPO_TOKEN` is configured, matching the AdventureMods release flow.
Flatpak packages include the narrow host bridge used for Flatpak lifecycle
operations; all other host managers continue to fail closed.

The APT reader and bundled libraries keep their original licenses. Package
scripts include dependency notices alongside PkgDeck's MIT license.
