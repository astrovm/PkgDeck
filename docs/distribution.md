# Packages and releases

## Homebrew

This repository is also a Homebrew tap. It has two prebuilt packages, so
nothing compiles on your machine:

| System | Package | Installs |
| --- | --- | --- |
| macOS 26+ | cask `pkgdeck` | `PkgDeck.app` in `/Applications`, plus `pkgdeck` and `pkd` commands |
| Linux | formula `pkd` | The `pkd` CLI |

Install the latest release:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck   # macOS
brew install astrovm/pkgdeck/pkd       # Linux
```

The full URL is needed because the repository isn't named `homebrew-pkgdeck`.
This is our own tap, not part of Homebrew core. Both recipes are rendered from
[`packaging/homebrew`](../packaging/homebrew) when a release is published.

### macOS

[`scripts/bundle-macos.sh`](../scripts/bundle-macos.sh) builds a self-contained
`PkgDeck.app` with Homebrew's Qt, a pinned Kirigami and `macdeployqt`, and
checks that it loads nothing from Homebrew. CI builds it on macOS 26 runners
for Apple silicon and Intel, so the app needs macOS 26 or later.

The app is ad-hoc signed, not notarized, because notarization needs a paid
Apple Developer ID. The cask removes the download quarantine after checking the
release checksum, so Gatekeeper doesn't block the first launch.

Before 0.1.10 the tap had a `pkgdeck` formula that built the app from source.
If you installed that on macOS, switch to the cask once:

```sh
brew uninstall --formula pkgdeck
brew install astrovm/pkgdeck/pkgdeck
```

### Linux

[`scripts/package-cli.sh`](../scripts/package-cli.sh) builds static (musl)
`pkd` and host runner binaries, so one archive per architecture works on any
distribution. The optional APT helper links the distro's `libapt-pkg`, so it
isn't included; APT is unavailable in this build but other package managers
still work. Use the Flatpak, Snap or AppImage for APT support. The formula
never installs system packages or runs Homebrew as root. Existing installs of
the old `pkgdeck` formula are renamed to `pkd` on `brew update`.

### Releasing a new version

1. Bump the workspace version in a pull request and wait for CI to pass,
   including `Test / Homebrew`, which installs the app and CLI from that
   build's packages.
2. After merging, tag the merged commit on `main`. Never reuse or move a
   published tag. The release workflow checks that the tagged code matches the
   tested pull request, then publishes once packaging passes.
3. Publishing the GitHub release runs `Publish / Homebrew`. It opens a PR that
   points the formula and cask at the release packages and their SHA-256
   checksums. CI installs and tests both from the published release.
4. GitHub squash-merges the formula PR once all checks pass. If a check fails,
   the PR stays open for review. After `brew update`, users get the new version.

Publishing needs the `HOMEBREW_REPO_TOKEN` secret, a fine-grained token for
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
- If you have the v0.1.1 AppImage, download v0.1.2 by hand once. Its built-in
  updater looks for an old file name. From v0.1.2 on, updates work
  automatically.
- The Snap is only published on GitHub, not the Snap Store.

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

Publishing needs the `FLATPAK_REPO_TOKEN` secret, a fine-grained token for
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
