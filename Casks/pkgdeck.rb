cask "pkgdeck" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.11.5"
  sha256 arm:   "175af9cc7449cdd9bddbcc9367967991194e9798c7debec20968e892c50df99c",
         intel: "d4eea9e5401b15fbe13916f163db623d04b9b7c5fe068d93b8052cd2574a0750"

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
