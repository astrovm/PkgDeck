# PkgDeck

A desktop app and command-line tool for managing packages across multiple sources.
Compare apps, manage repositories, and install updates from one place. Both
frontends use the same Rust engine and your existing package managers.

![PkgDeck search comparing application sources](docs/screenshots/search.png)

## Install

On Linux, install the Flatpak from the [astrovm Flatpak repository](https://github.com/astrovm/flatpak) with [Install PkgDeck](https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref), or use the terminal:

```sh
flatpak install https://flatpak.4st.li/io.github.astrovm.PkgDeck.flatpakref
```

Open PkgDeck from your application menu, or run `flatpak run io.github.astrovm.PkgDeck`. Updates arrive through your software manager or `flatpak update`.

Other Linux packages are available from [GitHub Releases](https://github.com/astrovm/PkgDeck/releases/latest):

| Package | How to run it |
| --- | --- |
| AppImage | Download `PkgDeck-x86_64.AppImage` or `PkgDeck-aarch64.AppImage`, run `chmod +x PkgDeck-<arch>.AppImage`, then `./PkgDeck-<arch>.AppImage`. |
| Snap | Download the `.snap` for your architecture, then run `sudo snap install --dangerous --classic ./pkgdeck_*.snap`. The Snap is hosted on GitHub, not the Snap Store. |

On macOS, install the GUI and CLI with Homebrew:

```sh
brew tap astrovm/pkgdeck https://github.com/astrovm/PkgDeck
brew install astrovm/pkgdeck/pkgdeck
```

Run `pkgdeck` from a terminal, or see [distribution](docs/distribution.md#homebrew) for the Applications shortcut. On Linux, the Homebrew formula installs only `pkd`; use a Linux package above for the GUI.
The CLI is `pkd` on Homebrew, `pkgdeck.pkd` in Snap,
`flatpak run --command=pkd io.github.astrovm.PkgDeck` in Flatpak, and
`./PkgDeck-x86_64.AppImage --cli` in AppImage (substitute your architecture).

To build on Linux from source, install the [development prerequisites](docs/development.md#sdk-and-prerequisites), then:

```sh
source scripts/dev-env.sh
cargo run --locked -p pkgdeck
```

## Use

- **Search:** compare the same app across sources and install a specific variant.
- **Installed:** filter packages, inspect details, and find duplicate installations.
- **Updates:** update individual packages or a selection, including firmware on Linux.
- **Clean:** preview and confirm manager-native cleanup tasks.
- **Sources:** choose managers and manage Flatpak repositories and firmware remotes.
- **Container images:** inspect, pull, refresh, and remove Docker or rootless Podman images.

The CLI works without Qt:

```sh
cargo run --locked -p pkd -- sources
pkd search vlc --from flatpak
pkd list --from apt
pkd upgrade --from flatpak
```

`pkd update` refreshes metadata; `pkd upgrade` updates installed packages.
Writes require confirmation. Native managers handle dependencies and authorization.
For APT, Update all previews a full `dist-upgrade` transaction. The GUI shows
planned installs and removals before confirmation. The CLI requires
`--allow-removals` if APT plans to remove packages, even with `--yes`.

## Package sources

PkgDeck detects the managers available on the host. Supported adapters include
APT, DNF, Pacman, Zypper, Flatpak, Snap, Homebrew formulae and macOS casks,
AppImage, Docker, Podman, Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer,
RubyGems, and fwupd. Capabilities vary by
manager; `pkd sources` reports availability and unsupported operations.
Standalone Codex, Claude Code, Grok, and OpenCode installations also support
updates through their upstream updaters; see the [CLI guide](docs/cli.md#standalone-cli-tools).
Linux-specific managers and firmware support are not available on macOS.

Native, AppImage, and classic Snap builds can manage host packages. The Flatpak
build manages host packages through its explicit host bridge.

## Documentation

- [GUI guide](docs/gui.md)
- [CLI commands, selection, and JSON output](docs/cli.md)
- [Development and verification](docs/development.md)
- [Distribution and releases](docs/distribution.md)
- [Shared engine](docs/shared-engine.md)
- [Host execution and authorization](docs/host-execution.md)

## Development

```sh
scripts/verify.sh fast --engine podman
scripts/verify.sh full --engine podman
```

Tests use synthetic packages; real package lifecycles run in disposable containers
and virtual machines. See the development guide for focused checks and native setup.

Licensed under [MIT](LICENSE). The optional APT reader and bundled dependencies
retain their own licenses; package scripts include the relevant notices.
