class Terminal < Formula
  desc "A fast, GPU-accelerated terminal emulator written in Rust"
  homepage "https://github.com/user/terminal"
  url "https://github.com/user/terminal/archive/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_ACTUAL_SHA256"
  license "MIT"

  depends_on "rust" => :build
  depends_on "molten-vk" => :recommended

  def install
    system "cargo", "install", "--locked", "--path", ".", "--root", prefix.to_s
  end

  test do
    assert_match "Terminal", shell_output("#{bin}/terminal --version", 1)
  end
end
