# Platform test audit

Reviewed 2026-09-26 for PR #114. Supported application platforms are Linux and
macOS on x86_64 and aarch64. This document describes the CI coverage contract;
the checks on the exact PR commit determine whether it passed. Windows is not
a supported build target.

## Shared and platform-specific suites

| Suite | Linux x86_64 | Linux aarch64 | macOS x86_64 | macOS aarch64 |
| --- | --- | --- | --- | --- |
| Core and CLI unit/integration tests | All | All | All | All |
| Real CLI terminal confirmation | GNU `script` | GNU `script` | Apple `script` | Apple `script` |
| Controller and GUI Rust tests | All | All | All applicable | All applicable |
| QML component/browser tests | Offscreen | Offscreen | Offscreen | Offscreen |
| GUI startup, broken-QML exit, single-instance forwarding, HTTPS/WebP cache | Yes | Yes | Yes | Yes |
| Native window keyboard lifecycle | Xvfb/xdotool | Xvfb/xdotool | Not automated | Not automated |
| Real sudo/polkit and APT lock handling | Yes | Yes | Not applicable | Not applicable |
| AppImage/Snap/Flatpak packaged GUI and host bridge | Yes | Yes | Not applicable | Not applicable |
| Homebrew packaged app/CLI | CLI | Container CLI lifecycle | App + CLI | App + CLI |
| Native XML/binary plist, application folders and cask ownership | Not applicable | Not applicable | Yes | Yes |

Native dpkg and AppImage execution tests, and the X11 keyboard driver, run only
on Linux. X11 testing does not establish native Cocoa interaction coverage. The macOS GUI
tests use the real Qt/Kirigami build installed by the Homebrew packaging job.
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
- Mac QML component tests use the Basic style selected by the shipped Mac
  launcher, avoiding native controls that reject the app's customization.
- Negated shell commands outside conditionals were not enforced by `set -e`.
  Removal checks now explicitly fail when native tools still report the package.

## Remaining validation boundaries

Passing CI is not exhaustive testing. Native Cocoa keyboard/accessibility
interaction, real firmware devices, real container image-source lifecycles,
signed vendor app/channel/architecture validation, multiple Homebrew prefixes,
and adoption/recovery workflows are not established by this matrix. The latter
write operations remain unsupported. Keep these gaps explicit when releasing;
do not label fixture-only checks as live backend tests.
