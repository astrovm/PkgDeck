cask "pkgdeck" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.12.5"
  sha256 arm:   "884ee3e0a0da9da3de90cca3ef638de458f07d7c8df43c884a97e766c4e05408",
         intel: "8580aa92b6711a80cc6e89d239ee8f1c26633cce515b8cafcc9838bfb9e248c3"

  url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-macos-#{arch}.zip"
  name "PkgDeck"
  desc "Browse and manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"

  depends_on macos: :tahoe

  app "PkgDeck.app"
  binary "#{appdir}/PkgDeck.app/Contents/MacOS/pkd"
  binary "#{appdir}/PkgDeck.app/Contents/MacOS/pkgdeck"

  # The app is not notarized, so Gatekeeper would refuse the
  # quarantined download; the checksum above verifies it instead.
  postflight_steps do
    run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{appdir}}/PkgDeck.app"]
  end

  zap trash: [
    "~/.cache/pkgdeck",
    "~/.local/state/pkgdeck",
    "~/Library/Caches/astrovm/PkgDeck",
    "~/Library/Caches/pkgdeck",
    "~/Library/LaunchAgents/io.github.astrovm.PkgDeck.plist",
    "~/Library/Preferences/io.github.astrovm.PkgDeck.plist",
  ]
end
