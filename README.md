# PkgDeck

**One app for every package manager on your computer.**

Search, install, update, and clean up software in one place.
It also comes with a command-line tool, `pkd`, which runs on the same engine.

![PkgDeck showing search results](docs/screenshots/search.png)

## ⬇️ Install

| Platform | How |
| --- | --- |
| **Linux** (Flatpak, recommended) | [Install PkgDeck](https://flatpak.4st.li/apps/io.github.astrovm.PkgDeck/install/) with one click, or run the command below |
| **Linux** (AppImage, Snap) | Download from [GitHub Releases](https://github.com/astrovm/PkgDeck/releases/latest) |
| **macOS** (Homebrew) | Run the commands below |
| **Linux** (Homebrew, CLI only) | `brew install astrovm/pkgdeck/pkd` |

### Linux (Flatpak)

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

- **Open it** from your app menu, or run `flatpak run io.github.astrovm.PkgDeck`.
- **Updates** arrive through your software manager or `flatpak update`.

### macOS (Homebrew)

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew trust astrovm/pkgdeck
brew install astrovm/pkgdeck/pkgdeck
```

- **What you get:** PkgDeck.app in `/Applications` and the `pkd` CLI.
- **Needs** macOS 26 or later (it's prebuilt for it).
- **Why `brew trust`:** it lets `brew upgrade` update PkgDeck. Homebrew 6 and later skip taps you haven't trusted.
- **On Linux,** `brew install astrovm/pkgdeck/pkd` installs only the CLI.

## 🚀 Use

### The app

| Page | What it does |
| --- | --- |
| **Search** | Compare software across sources and choose the exact version and scope |
| **Installed** | Browse installed packages and spot duplicate installs |
| **Updates** | Apply one update, a selection, or everything at once |
| **Clean** | Preview what will be removed before you confirm |
| **Sources** | Turn package managers on or off and manage repositories |

### The `pkd` CLI

With Flatpak, run the CLI like this:

```sh
flatpak run --command=pkd io.github.astrovm.PkgDeck
```

With other installs, run `pkd` directly:

| Command | What it does |
| --- | --- |
| `pkd sources` | List available package managers |
| `pkd search vlc --from flatpak` | Search one source |
| `pkd list --from apt` | List installed packages |
| `pkd refresh` | Refresh package lists |
| `pkd upgrade` | Install available updates |

![pkd search showing ripgrep and related packages from APT and Flatpak](docs/screenshots/cli-search.png)

- **You stay in control.** PkgDeck asks you to confirm every change.
- **Passwords.** System-wide changes may ask for your password.
- **All commands** are in the [CLI guide](docs/cli.md).

## Supported sources

PkgDeck works with the package managers you already have, such as APT, Flatpak, Snap, Homebrew, developer tools, and container images.

Some features are only available for certain managers. See [supported sources](docs/gui.md) for details.

## Documentation

| Guide | About |
| --- | --- |
| [GUI guide](docs/gui.md) | Using the app |
| [CLI guide](docs/cli.md) | Every `pkd` command |
| [Development and tests](docs/development.md) | Working on PkgDeck |
| [Packages and releases](docs/distribution.md) | How it's packaged and shipped |
| [Host access and authorization](docs/host-execution.md) | How it runs commands on your system |

<details>
<summary><b>Build from source</b></summary>

Install the [prerequisites](docs/development.md#sdk-and-prerequisites), then run:

```sh
source scripts/dev-env.sh
cargo run --locked -p pkgdeck
```

</details>
