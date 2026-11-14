cask "pkgdeck" do
  arch arm: "aarch64", intel: "x86_64"

  version "0.15.3"
  sha256 arm:   "d80f7437c973ab3c1d33eab9a6e8ba395f740ce2cd92372c9ad6377f31109476",
         intel: "e9e281fb5d32ef456cc8599e5efd1dce81abe462385a9da72da94b2ae13ec600"

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
