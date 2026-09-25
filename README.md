# PkgDeck

PkgDeck is one app for every package manager on your computer. Use it to search, install, update, and clean up software. It also comes with a command-line tool, `pkd`, which runs on the same engine.

![PkgDeck showing search results](docs/screenshots/search.png)

## Features

| Page          | What it does                                                                |
| ------------- | --------------------------------------------------------------------------- |
| **Search**    | Compare software across sources and choose the exact version and scope      |
| **Installed** | Browse installed packages and spot duplicate installs                       |
| **Updates**   | Apply one update, a selection, or everything at once                        |
| **Clean**     | Preview what will be removed before you confirm                             |
| **Sources**   | Turn package managers on or off and manage repositories                     |

PkgDeck works with the package managers you already have, such as APT, Flatpak, Snap, Homebrew, developer tools, and container images. Some features are only available for certain managers. See [supported sources](docs/gui.md) for details.

## Install

### Linux (Flatpak, recommended)

[Install PkgDeck](https://flatpak.4st.li/apps/io.github.astrovm.PkgDeck/install/) with one click, or run:

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

Open PkgDeck from your app menu, or run `flatpak run io.github.astrovm.PkgDeck`. Updates arrive through your software manager or `flatpak update`.

[GitHub Releases](https://github.com/astrovm/PkgDeck/releases/latest) also has AppImage and Snap packages.

### macOS (Homebrew)

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck
```

This installs both the app and the `pkd` CLI. On Linux, the same formula installs only the CLI.

## Command line

With Flatpak, run the CLI like this:

```sh
flatpak run --command=pkd io.github.astrovm.PkgDeck
```

With other installs, run `pkd` directly:

```sh
pkd sources                      # list available package managers
pkd search vlc --from flatpak    # search one source
pkd list --from apt              # list installed packages
pkd update                       # refresh package metadata
pkd upgrade                      # install available updates
```

PkgDeck asks you to confirm every change, and system-wide changes may ask for your password. See the [CLI guide](docs/cli.md) for all commands.

## Build from source

Install the [prerequisites](docs/development.md#sdk-and-prerequisites), then run:

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
