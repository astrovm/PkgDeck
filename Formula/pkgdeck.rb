class Pkgdeck < Formula
  desc "Browse and manage packages across native package managers"
  homepage "https://github.com/astrovm/PkgDeck"
  url "https://github.com/astrovm/PkgDeck/archive/refs/tags/v0.1.3.tar.gz"
  version "0.1.3"
  sha256 "6c3a83ed195d248a7311c01596a49fad4377b7769a0023c7cab8b02e981404c5"
  license "MIT"
  head "https://github.com/astrovm/PkgDeck.git", branch: "main"

  depends_on "rust" => :build

  on_macos do
    depends_on "cmake" => :build
    depends_on "librsvg" => :build
    depends_on "ninja" => :build
    depends_on "qtshadertools" => :build
    depends_on "qttools" => :build
    depends_on "qtbase"
    depends_on "qtdeclarative"
    depends_on "qtimageformats"
    depends_on "qtsvg"

    resource "extra-cmake-modules" do
      url "https://download.kde.org/stable/frameworks/6.24/extra-cmake-modules-6.24.0.tar.xz"
      sha256 "8ef3f7e176588e099c02559d20ddf4fed0590f92c168f0bcc60a7e638ba1e6a3"
    end

    resource "kirigami" do
      url "https://download.kde.org/stable/frameworks/6.24/kirigami-6.24.0.tar.xz"
      sha256 "7b3247dfe349867d44244335beb8d549ad4a8f6b3179d1736d231512dea5b0ce"
    end
  end

  def install
    if OS.mac?
      resource("extra-cmake-modules").stage do
        system "cmake", "-S", ".", "-B", "build", "-G", "Ninja",
               *std_cmake_args(install_prefix: buildpath/"ecm"), "-DBUILD_TESTING=OFF"
        system "cmake", "--build", "build"
        system "cmake", "--install", "build"
      end
      resource("kirigami").stage do
        (pkgshare/"licenses/kirigami").install Dir["LICENSES/*"]
        system "cmake", "-S", ".", "-B", "build", "-G", "Ninja",
               *std_cmake_args(install_prefix: libexec/"kirigami"),
               "-DECM_DIR=#{buildpath}/ecm/share/ECM/cmake",
               "-DKDE_INSTALL_QMLDIR=lib/qml", "-DKDE_INSTALL_LIBDIR=lib",
               "-DBUILD_TESTING=OFF", "-DBUILD_EXAMPLES=OFF", "-DBUILD_QCH=OFF"
        system "cmake", "--build", "build"
        system "cmake", "--install", "build"
      end
      ENV["QMAKE"] = formula_opt_bin("qtbase")/"qmake"
      system "cargo", "install", *std_cargo_args(path: "crates/pkgdeck", root: buildpath/"gui")
      app = prefix/"PkgDeck.app/Contents"
      (app/"MacOS").install "gui/bin/pkgdeck" => "pkgdeck-bin"
      iconset = buildpath/"PkgDeck.iconset"
      iconset.mkpath
      [[16, "16x16"], [32, "16x16@2x"], [32, "32x32"], [64, "32x32@2x"],
       [128, "128x128"], [256, "128x128@2x"], [256, "256x256"],
       [512, "256x256@2x"], [512, "512x512"], [1024, "512x512@2x"]].each do |size, name|
        system "rsvg-convert", "-w", size.to_s, "-h", size.to_s,
               "assets/io.github.astrovm.PkgDeck.svg", "-o", iconset/"icon_#{name}.png"
      end
      (app/"Resources").mkpath
      system "iconutil", "-c", "icns", iconset, "-o", app/"Resources/PkgDeck.icns"
      (app/"Info.plist").write <<~XML
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0"><dict>
          <key>CFBundleIdentifier</key><string>io.github.astrovm.PkgDeck</string>
          <key>CFBundleName</key><string>PkgDeck</string>
          <key>CFBundleExecutable</key><string>pkgdeck</string>
          <key>CFBundleIconFile</key><string>PkgDeck</string>
          <key>CFBundlePackageType</key><string>APPL</string>
          <key>CFBundleShortVersionString</key><string>#{File.read("Cargo.toml")[/^version = "([^"]+)"$/, 1]}</string>
          <key>NSHighResolutionCapable</key><true/>
        </dict></plist>
      XML
      (app/"MacOS/pkgdeck").write <<~SH
        #!/bin/sh
        export PATH="#{HOMEBREW_PREFIX}/bin:#{HOMEBREW_PREFIX}/sbin:$PATH"
        export QML_IMPORT_PATH="#{opt_libexec}/kirigami/lib/qml${QML_IMPORT_PATH:+:$QML_IMPORT_PATH}"
        export QT_QUICK_CONTROLS_STYLE=Basic
        exec "#{opt_prefix}/PkgDeck.app/Contents/MacOS/pkgdeck-bin" "$@"
      SH
      chmod 0755, app/"MacOS/pkgdeck"
      bin.install_symlink app/"MacOS/pkgdeck"
    end
    system "cargo", "install", *std_cargo_args(path: "crates/pkd")
    if OS.linux?
      system "cargo", "install", *std_cargo_args(path: "crates/pkgdeck-core", root: buildpath/"runner")
      libexec.install "runner/bin/pkgdeck-host-runner"
      # Preserve the optional distro APT reader outside Cargo's temporary build tree.
      Dir["target/release/build/pkgdeck-core-*/out/pkgdeck-apt-query"].each do |helper|
        bin.install helper
        (pkgshare/"licenses").install "native/COPYING" => "APT-HELPER-GPL-2"
      end
    end
  end

  def caveats
    return unless OS.mac?

    <<~EOS
      Launch the GUI with pkgdeck, or link it into your user Applications folder:
        mkdir -p ~/Applications
        ln -s #{opt_prefix}/PkgDeck.app ~/Applications/PkgDeck.app
      The CLI is available as pkd.
    EOS
  end

  test do
    assert_match "pkd", shell_output("#{bin}/pkd --version")
    assert_match "Usage:", shell_output("#{bin}/pkd --help")
    if OS.mac?
      assert_match "pkgdeck", shell_output("#{bin}/pkgdeck --version")
      assert_match "PKGDECK_GUI_READY", shell_output("#{bin}/pkgdeck --smoke-test 2>&1")
    else
      refute_path_exists bin/"pkgdeck"
      assert_path_exists libexec/"pkgdeck-host-runner"
    end
  end
end
