class Pkd < Formula
  desc "Command-line tool to manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"
  version "0.3.0"
  license "MIT"

  # macOS gets pkd with the app: brew install astrovm/pkgdeck/pkgdeck
  depends_on :linux

  if Hardware::CPU.arm?
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-aarch64-cli.tar.gz"
    sha256 "5b9975870bd12a003876d3ee33b9abe86477760da1bfa8328aed18368500f6ce"
  else
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-x86_64-cli.tar.gz"
    sha256 "d8ee57fcc3e5b34fda290b76ebf7eacff67c42849052d4b0f44de104c8beece1"
  end

  def install
    bin.install "bin/pkd"
    libexec.install "libexec/pkgdeck-host-runner"
  end

  test do
    assert_match "pkd", shell_output("#{bin}/pkd --version")
    assert_match "Usage:", shell_output("#{bin}/pkd --help")
    assert_path_exists libexec/"pkgdeck-host-runner"
    # Host package managers can fail detection inside the test sandbox,
    # which exits 1; any other status means the command itself failed.
    sources = shell_output("#{bin}/pkd --json sources; echo \"exit=$?\"")
    opoo sources unless sources.end_with?("exit=0\n")
    assert_match(/exit=[01]\n\z/, sources)
  end
end
