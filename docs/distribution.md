# Packages and releases

## Homebrew

This repository is also a Homebrew tap. The formula is
[`Formula/pkgdeck.rb`](../Formula/pkgdeck.rb).

| System | Commands | App |
| --- | --- | --- |
| macOS | `pkd`, `pkgdeck` | `PkgDeck.app` |
| Linux | `pkd` | Not included |

Install the latest release:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck
```

The full URL is needed because the repository isn't named `homebrew-pkgdeck`.
This is our own tap, not part of Homebrew core.

To build the latest development version instead:

```sh
brew install --HEAD astrovm/pkgdeck/pkgdeck
brew upgrade --fetch-HEAD astrovm/pkgdeck/pkgdeck   # update it later
```

### macOS

Homebrew provides Qt. The formula builds a pinned copy of Kirigami and installs
a launcher that sets the right QML paths and Qt style. Run `pkgdeck` from a
terminal, or add the app to Finder:

```sh
mkdir -p ~/Applications
ln -s "$(brew --prefix astrovm/pkgdeck/pkgdeck)/PkgDeck.app" ~/Applications/PkgDeck.app
```

The formula builds from source (there's no bottle). The `.app` depends on
Homebrew, so it isn't a standalone macOS download.

### Linux

On Linux, the formula only builds the CLI. APT support needs the
`pkgdeck-apt-query` helper, which is built only if your distro's
`libapt-pkg-dev` is installed at build time. Without it, APT is unavailable
but other package managers still work. The formula never installs system
packages or runs Homebrew as root.

CI builds and tests the formula on Linux and macOS. The macOS build must pass
before the macOS package is considered supported. A Linux build alone doesn't
count.

### Releasing a new version

1. Bump the workspace version in a pull request and wait for CI to pass. Check
   the macOS GUI test and the Linux check that no GUI command was installed.
2. After merging, tag the merged commit on `main`. Never reuse or move a
   published tag. The release workflow checks that the tagged code matches the
   tested pull request, then publishes once packaging passes.
3. Publishing the GitHub release runs `Publish / Homebrew`. It opens a PR that
   updates the formula's archive URL and SHA-256, keeping `head` for
   development builds. CI tests the updated formula on Linux and macOS.
4. GitHub squash-merges the formula PR once all checks pass. If a check fails,
   the PR stays open for review. After `brew update`, users get the new version.

**Required secret:** `HOMEBREW_REPO_TOKEN`, a fine-grained token for
`astrovm/PkgDeck` with Contents and Pull requests write access. It lets the
formula PR trigger CI and auto-merge. Publishing fails with a clear error if
it's missing.

## Linux packages

Most users should install the Flatpak from the signed
[astrovm Flatpak repository](https://github.com/astrovm/flatpak):

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

The `.flatpakref` also adds the repository, so you get updates. The `.flatpak`
files on GitHub Releases are standalone bundles.

GitHub Releases also have AppImage and Snap packages:

- Every file is named `PkgDeck-v<version>-…`, including checksums and Flatpak
  metadata. Linux packages end with `x86_64` or `aarch64` before the extension.
- **AppImage:** if you have v0.1.1, download v0.1.2 by hand once. Its built-in
  updater looks for an old file name. From v0.1.2 on, updates work
  automatically.
- **Snap:** only published on GitHub, not the Snap Store.

To build the packages, use the same environment as CI:

```sh
scripts/verify.sh full --engine podman
```

See [`scripts/package.sh`](../scripts/package.sh),
[`scripts/bundle.sh`](../scripts/bundle.sh), and the packaging jobs in
[CI](../.github/workflows/ci.yml) for exact commands and checks.

### How releases are built

- Every pull request runs the full test suite and builds and tests the Linux
  packages on both architectures.
- A `v*` tag must point to a merged pull request commit on `main` with the same
  code and a passing CI run.
- The tag workflow builds and tests the release packages, then creates a GitHub
  release with AppImage update files, Flatpak bundles, Snap packages,
  checksums, and provenance. It doesn't rerun the full test suite or Homebrew
  builds.
- `Publish Flatpak repository` sends the release bundles to
  [`astrovm/flatpak`](https://github.com/astrovm/flatpak).

**Required secret:** `FLATPAK_REPO_TOKEN`, a fine-grained token for
`astrovm/flatpak` with Contents write access. Publishing fails with a clear
error if it's missing. To republish an existing release, run `publish.yml` in
`astrovm/flatpak` manually with `repository=astrovm/PkgDeck` and the release
tag.

The Flatpak manages your system's package managers through a host bridge. It
has host file access and uses the same confirmation and password rules as a
native install.

## Licenses

The APT reader and bundled libraries keep their own licenses. The package
scripts include their notices alongside PkgDeck's MIT license.
