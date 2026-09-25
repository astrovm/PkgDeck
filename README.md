# PkgDeck

PkgDeck brings your package managers into one desktop app. Search for software, see what is installed, review updates, and clean up unused packages. A command-line tool, `pkd`, uses the same engine.

![PkgDeck showing search results](docs/screenshots/search.png)

## Install

[Install PkgDeck](https://flatpak.4st.li/apps/io.github.astrovm.PkgDeck/install/), or use the terminal:

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

Open PkgDeck from your app menu or run:

```sh
flatpak run io.github.astrovm.PkgDeck
```

Flatpak updates arrive through your software manager or `flatpak update`. [GitHub Releases](https://github.com/astrovm/PkgDeck/releases/latest) also provides AppImage and Snap packages for Linux.

On macOS, install the GUI and CLI with Homebrew:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck
```

On Linux, the Homebrew formula installs the CLI only. Use Flatpak, AppImage, or Snap for the GUI.

## Use

- **Search:** compare software across available sources and choose the exact version or installation scope.
- **Installed:** find packages and duplicate installs.
- **Updates:** review and apply one update, a selection, or all available updates.
- **Clean:** preview cleanup before confirming it.
- **Sources:** enable package managers and manage repositories.

PkgDeck uses the package managers already on your computer, including APT, Flatpak, Snap, Homebrew, developer tools, and container images. Available features depend on each manager. See [supported sources](docs/gui.md) for details.

## Command line

The Flatpak includes `pkd`:

```sh
flatpak run --command=pkd io.github.astrovm.PkgDeck
```

In native builds, run `pkd` directly. For example:

```sh
pkd sources
pkd search vlc --from flatpak
pkd list --from apt
```

`pkd update` refreshes package metadata; `pkd upgrade` installs available updates. Changes require confirmation, and system changes may ask for authorization. See the [CLI guide](docs/cli.md) for exact selectors and more commands.

## Build from source

Install the [development prerequisites](docs/development.md#sdk-and-prerequisites), then:

```sh
source scripts/dev-env.sh
cargo run --locked -p pkgdeck
```

## Documentation

- [GUI guide](docs/gui.md)
- [CLI guide](docs/cli.md)
- [Development and tests](docs/development.md)
- [Packages and releases](docs/distribution.md)
- [Host access and authorization](docs/host-execution.md)

Licensed under [MIT](LICENSE). The optional APT reader and bundled dependencies retain their own licenses; package scripts include their notices.
