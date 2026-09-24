# Distribution

## Homebrew

The repository doubles as a third-party Homebrew tap. Its formula lives in
[`Formula/pkgdeck.rb`](../Formula/pkgdeck.rb).

| Host | Installed commands | GUI |
| --- | --- | --- |
| macOS | `pkd`, `pkgdeck` | `PkgDeck.app` |
| Linux | `pkd` | Not installed by Homebrew |

Install the current stable release with:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck
```

Use the explicit repository URL: this project is not named `homebrew-pkgdeck`.
This is our tap, not a submission to Homebrew core. To build the current
development branch instead, use `brew install --HEAD astrovm/pkgdeck/pkgdeck`.
To update a HEAD installation:

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

1. Set the workspace version in a pull request and let CI pass. Review the
   macOS Homebrew GUI smoke result and the Linux check that no GUI command is
   installed.
2. Tag that pull request's merged commit on `main`. Do not reuse or move a
   published tag. The tag workflow checks that the merged source tree matches
   the tested pull request and publishes only after that check and packaging pass.
3. Publishing the GitHub release triggers `Publish / Homebrew`, which opens a PR
   with the immutable archive URL and SHA-256 while retaining `head` for development.
   Pull request CI builds and tests the published formula on Linux and macOS.
4. GitHub automatically squash-merges the formula PR after every required check
   passes. A failed check leaves the PR open for review. After users run
   `brew update`, `brew install astrovm/pkgdeck/pkgdeck` installs the stable version.

Configure `HOMEBREW_REPO_TOKEN` as a PkgDeck Actions secret using a fine-grained
token restricted to `astrovm/PkgDeck` with Contents and Pull requests write
permissions. It lets formula PRs created by the release workflow start CI and
merge automatically. Publication fails clearly if the secret is missing.

The formula builds from the tagged source archive; it does not provide a bottle.
The `.app` uses Homebrew dependencies and is not a standalone downloadable
macOS application.

## Linux packages

Users can install the Flatpak from the signed [astrovm Flatpak repository](https://github.com/astrovm/flatpak) with:

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

The `.flatpakref` adds the repository for future updates. GitHub release
`.flatpak` files are standalone bundles.

The packaging scripts build AppImage, Flatpak, and Snap artifacts. Every GitHub
release asset starts with `PkgDeck-v<version>-`, including checksums and Flatpak
metadata. Linux packages add `x86_64` or `aarch64` before the extension.
AppImages downloaded from v0.1.1 need one manual download of v0.1.2 because
their embedded updater expects the old unversioned `.zsync` filename. AppImages
from v0.1.2 onward follow the versioned filename pattern automatically.
Use the same verified build environment as CI:

```sh
scripts/verify.sh full --engine podman
```

See [`scripts/package.sh`](../scripts/package.sh),
[`scripts/bundle.sh`](../scripts/bundle.sh), and the packaging jobs in
[CI](../.github/workflows/ci.yml) for the supported invocations and artifact checks.

Ordinary pull request CI runs the full test matrix and builds and smoke-tests
the Linux packages on both architectures. A `v*` tag must point to a merged
pull request commit on `main` with the same source tree and a successful
pull request CI run.
The tag workflow then builds and tests the release packages, creates a GitHub
release with AppImage update metadata, Flatpak bundles, Snap packages,
checksums, and provenance. It does not repeat the full test matrix or Homebrew
builds. Snap packages are published on GitHub only; no Snap Store upload runs.
`Publish Flatpak repository` dispatches the immutable
release bundles to [`astrovm/flatpak`](https://github.com/astrovm/flatpak).
Configure `FLATPAK_REPO_TOKEN` as a PkgDeck Actions secret using a fine-grained
token restricted to `astrovm/flatpak` with Contents write permission. The
publication job fails clearly if it is missing.
For an existing release, manually dispatch `publish.yml` in `astrovm/flatpak` with
`repository=astrovm/PkgDeck` and the release tag. Flatpak packages use the host bridge for
all supported managers, with explicit host filesystem access and the same
confirmation and authorization rules as native execution.

The APT reader and bundled libraries keep their original licenses. Package
scripts include dependency notices alongside PkgDeck's MIT license.
