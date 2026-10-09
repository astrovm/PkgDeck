class Pkd < Formula
  desc "Command-line tool to manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"
  version "0.7.0"
  license "MIT"

  # macOS gets pkd with the app: brew install astrovm/pkgdeck/pkgdeck
  depends_on :linux

  if Hardware::CPU.arm?
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-aarch64-cli.tar.gz"
    sha256 "2fc25d47e86a20e90da7d6c6a22b7b949208ee43b37306e13125f1aafc7fce09"
  else
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-x86_64-cli.tar.gz"
    sha256 "6be7f896c1e3e7f9c166280e5c6f74ef1b7380bb9097c50f0fd398a3c837ad6f"
  end

  def install
    bin.install "bin/pkd"
    bin.install "bin/pkgdeck-apt-query"
    libexec.install "libexec/pkgdeck-host-runner"
    # pkd finds it at ../lib/pkgdeck from its own (resolved) path.
    (lib/"pkgdeck").install Dir["lib/pkgdeck/*"]
    (share/"licenses/pkgdeck").install "share/licenses/pkgdeck/apt"
  end

  test do
    assert_match "pkd", shell_output("#{bin}/pkd --version")
    assert_match "Usage:", shell_output("#{bin}/pkd --help")
    assert_path_exists libexec/"pkgdeck-host-runner"
    # The helper uses its private loader and libraries, even when the system
    # has a different APT ABI. It reads the host database without changing it.
    if which("apt-get")
      arch = Hardware::CPU.arm? ? "arm64" : "amd64"
      apt = JSON.parse(shell_output("#{bin}/pkd --json --from apt --arch #{arch} info bash"))
      assert_equal "bash", apt.fetch("data").fetch("package").fetch("id").fetch("name")
      refute_nil apt.fetch("data").fetch("package").fetch("installed_version")
    end
    updater = lib/"pkgdeck/appimageupdatetool.AppImage"
    assert_predicate updater, :executable?
    assert_match "appimageupdatetool version", shell_output("#{updater} --appimage-extract-and-run --version 2>&1")
    # Host package managers can fail detection inside the test sandbox,
    # which exits 1; any other status means the command itself failed.
    sources = shell_output("#{bin}/pkd --json sources; echo \"exit=$?\"")
    opoo sources unless sources.end_with?("exit=0\n")
    assert_match(/exit=[01]\n\z/, sources)
  end
end
