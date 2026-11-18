class Pkd < Formula
  desc "Command-line tool to manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"
  version "0.15.5"
  license "MIT"

  # macOS gets pkd with the app: brew install astrovm/pkgdeck/pkgdeck
  depends_on :linux

  if Hardware::CPU.arm?
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-aarch64-cli.tar.gz"
    sha256 "f0b31c0fc9a6cfbd2a2386151edb2dfacf7bb5c54302e8f70eb6e562afd2a765"
  else
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-x86_64-cli.tar.gz"
    sha256 "deb098cdc7f23a91c54f340c9bdf61fb4711e1d65c4fbbde4bcce3b87bafcf30"
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
    # Host package managers can fail detection inside the test sandbox, such
    # as Cargo, whose rustup has no default toolchain under the sandbox's HOME.
    # pkd exits 1 exactly when a source failed and names it in the report.
    output = shell_output("#{bin}/pkd --json sources; echo \"exit=$?\"")
    report, status = output.match(/\A(.*)exit=(\d+)\n\z/m).captures
    sources = JSON.parse(report).fetch("data").fetch("sources")
    refute_empty sources
    failed = sources.select { |source| source.fetch("availability").key?("Err") }.map { |source| source["backend"] }
    assert_equal failed.empty? ? "0" : "1", status
    ohai "Not checked in the test sandbox: #{failed.join(", ")}" unless failed.empty?
  end
end
