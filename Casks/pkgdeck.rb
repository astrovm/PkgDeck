cask "pkgdeck" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.4.2"
  sha256 arm:   "ed43e1eb717d9cc712fff2f5d4f9256f05a59ee2491e5a43d79665ed190da934",
         intel: "57523d3bfd729151f19bb447de2e69f01623cb0f7c7b9b094cb8cd1cc62522c1"

  url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-macos-#{arch}.zip"
  name "PkgDeck"
  desc "Browse and manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"

  depends_on macos: :tahoe

  app "PkgDeck.app"
  binary "#{appdir}/PkgDeck.app/Contents/MacOS/pkd"
  binary "#{appdir}/PkgDeck.app/Contents/MacOS/pkgdeck"

  # The app is ad-hoc signed, not notarized, so Gatekeeper would refuse the
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
