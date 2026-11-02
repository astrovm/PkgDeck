cask "pkgdeck" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.12.9"
  sha256 arm:   "dcd1239250c5b84605dab2d6c0933be2abd5170accb902456e3c5c832877ea57",
         intel: "4d46a659dbb27b0a1411bea6721ff66aa79aba4b336a608a7b73a72dd13c7d7e"

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
