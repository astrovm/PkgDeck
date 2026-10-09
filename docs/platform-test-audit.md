# Platform test audit

Reviewed 2026-09-26 for PR #114; packaging rows updated 2026-09-27 for PR #118. Supported application platforms are Linux and
macOS on x86_64 and aarch64. This document describes the CI coverage contract;
the checks on the exact PR commit determine whether it passed. Windows is not
a supported build target.

The GUI moved from Qt/QML to egui after this audit. The tables and notes below
describe the egui app. The findings are kept as they were found.

## Shared and platform-specific suites

| Suite | Linux x86_64 | Linux aarch64 | macOS x86_64 | macOS aarch64 |
| --- | --- | --- | --- | --- |
| Core and CLI unit/integration tests | All | All | All | All |
| Real CLI terminal confirmation | GNU `script` | GNU `script` | Apple `script` | Apple `script` |
| Controller and GUI Rust tests | All | All | All applicable | All applicable |
| Window logic against a fake package manager | All | All | All | All |
| Single-instance forwarding and HTTPS media cache | Yes | Yes | Yes | Yes |
| GUI startup | Xvfb | Xvfb | Cocoa smoke test | Cocoa smoke test |
| Clean exit with no display | Yes | Yes | Not applicable | Not applicable |
| Native window keyboard lifecycle | Xvfb/xdotool | Xvfb/xdotool | Not automated | Not automated |
| Real sudo/polkit and APT lock handling | Yes | Yes | Not applicable | Not applicable |
| AppImage/Snap/Flatpak packaged GUI and host bridge | Yes | Yes | Not applicable | Not applicable |
| Homebrew packages | Static CLI formula | Static CLI formula | App + CLI cask | App + CLI cask |
| Static CLI archive on another libc (Alpine) | Yes | Yes | Not applicable | Not applicable |
| Native XML/binary plist, application folders and cask ownership | Not applicable | Not applicable | Yes | Yes |

Native dpkg and AppImage execution tests, and the X11 keyboard driver, run only
on Linux. X11 testing does not establish native Cocoa interaction coverage. The macOS window tests
build the same `pkgdeck` that `scripts/bundle-macos.sh` bundles into the
shipped app. The Homebrew test installs that app through the cask and runs
`scripts/tests/macos-gui.sh` through the linked `pkgdeck` command. It starts
`pkgdeck --smoke-test` 15 times. Each run must print `PKGDECK_GUI_READY` and
build the menu bar menu (`PKGDECK_TRAY_MENU Open|Check now|Quit`). Cocoa
startup has a 60-second limit (180 seconds for the first run) and keeps process
samples on timeout. AppKit asks
IconServices for an icon on the main thread when the window takes focus, so the
check first probes IconServices from a separate process and skips with a warning
when the runner's daemon does not answer. The Linux formula test accepts a nonzero
`pkd sources` status when a host package manager fails detection in the sandbox.
GPU/Metal behavior is outside this hosted-runner check.
Mac-only code is compiled and exercised on native runners, not merely checked
through Linux fixtures.

## Backend execution coverage

| Sources | Real lifecycle | Shared fixtures and limitations |
| --- | --- | --- |
| APT | Both Linux architectures, including GUI/container and authorization boundaries | Structured metadata, ambiguity, locks, cancellation and native planning tests |
| DNF, Zypper | Both Linux architectures in their native distro containers | Parser/command/identity tests also run in the portable suites |
| Pacman | Linux x86_64 in the official Arch container | Official Arch container has no aarch64 variant; parser/command tests run on both CPUs |
| Snap, Flatpak | Both Linux architectures | Store/repository/network access remains an external CI dependency |
| Homebrew formulae | Both Linux architectures in lifecycle containers | Transport and metadata tests run on all four platforms; macOS package build exercises a real formula |
| Homebrew casks and macOS Applications | Both Mac architectures, with a locally built cask and actual `plutil` | Exact-copy ownership, duplicate bundles, receipts, corrupt metadata, directory permissions, symlinks, details, search and rejected writes |
| Cargo, npm, pnpm, Bun, pip, pipx, uv, Composer, RubyGems | Linux and macOS on both architectures | Independent jobs isolate each manager's environment; underlying manager queries verify the installed state |
| Docker, Podman image sources | Structured transport fixtures on all four platforms | Running a Podman package-test container does not itself prove the image-source lifecycle; native daemon lifecycle remains a separate gap |
| Codex, Claude Code, Grok, OpenCode standalone | Compiled executable fixtures, native process transport, all four platforms | No vendor credentials, paid requests or live self-updater downloads; these fixtures validate PkgDeck's ownership and command boundaries |
| Firmware | Structured fwupd fixtures | No real firmware flash on disposable CI runners; hardware validation is required |
| AppImage source | Real packaged execution on both Linux architectures plus discovery/import fixtures | Not a macOS executable format |

## Findings addressed in this audit

- macOS previously built the Homebrew package but did not run the Rust suites.
- The GUI source-name map omitted `macos-apps`; the full Linux suite caught it.
- Linux `/proc` liveness checks incorrectly treated active Mac history entries as
  interrupted. Recovery now uses a portable, non-signalling process probe, with
  live/dead owner assertions.
- CLI confirmation fixtures assumed GNU `script` syntax and `/bin/true` paths.
  They now use the native terminal recorder and portable executable locations.
- Symlink-loop reporting assumed Linux's numeric error code. It now uses the
  platform's error constant, with real symlink-loop coverage on both OSes.
- Read-only inventory failures no longer enter mutation planning. Exact package
  details only consult the macOS inventory for absolute `.app` identities.
- GUI lifecycle checks wait for the recorded operation outcome before closing,
  rather than treating a short CPU-idle interval as transaction completion.
  The driver runs as the application user and preserves native manager config,
  including Homebrew's trusted-tap settings.
- Mac QML component tests (from the Qt app) used the Basic style the shipped Mac app selected, avoiding native controls that rejected the app's customization.
  Banner tests verified that the Linux authorization shortcut was hidden on Mac
  and tested ordinary Settings navigation on both platforms.
- Applications removed during directory metadata or path resolution are
  reported as skipped entries without discarding the remaining inventory.
- Negated shell commands outside conditionals were not enforced by `set -e`.
  Removal checks now explicitly fail when native tools still report the package.

## Findings from PR #118

- AppImage registered without an availability probe, so every macOS search
  reported "AppImage requires Linux" as a failed source. It is now probed like
  every other source and left out of automatic queries on macOS.
- Homebrew still quarantines cask downloads, and Gatekeeper blocks the ad-hoc
  signed app. The cask clears quarantine after the checksum is verified; CI
  checks the installed app is not quarantined.
- The Qt app could not find its bundled plugins when it ran through Homebrew's
  `pkgdeck` symlink. The binary now re-runs itself from inside the bundle, and
  the Cocoa smoke test goes through that symlink.
- Static-binary checks rejected valid output: `file` prints "statically linked"
  on aarch64 and "static-pie linked" on x86_64.
- The packaged app, its Finder launch, source detection and search were also
  driven by hand on an Apple silicon Mac through the accessibility API. That is
  a one-off check, not automated Cocoa interaction coverage.

## Remaining validation boundaries

Passing CI is not exhaustive testing. Native Cocoa keyboard/accessibility
interaction, real firmware devices, real container image-source lifecycles,
signed vendor app/channel/architecture validation, multiple Homebrew prefixes,
and adoption/recovery workflows are not established by this matrix. The latter
write operations remain unsupported. Keep these gaps explicit when releasing;
do not label fixture-only checks as live backend tests.
