class Pkd < Formula
  desc "Command-line tool to manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"
  version "0.6.0"
  license "MIT"

  # macOS gets pkd with the app: brew install astrovm/pkgdeck/pkgdeck
  depends_on :linux

  if Hardware::CPU.arm?
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-aarch64-cli.tar.gz"
    sha256 "f93a42e52d05b1787e3fc5452f4a8a15ea81fb274bf5728ffdfa9036a3bc34c0"
  else
    url "https://github.com/astrovm/PkgDeck/releases/download/v#{version}/PkgDeck-v#{version}-linux-x86_64-cli.tar.gz"
    sha256 "a29ffcea11a6d13e6381045274497962a3f9f556b7181fdda48be7a18a9d9925"
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
