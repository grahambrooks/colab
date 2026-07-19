class Colab < Formula
  desc "Automated refactoring and project updates"
  homepage "https://github.com/grahambrooks/colab"
  version "2026.7.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/grahambrooks/colab/releases/download/v2026.7.0/colab-v2026.7.0-aarch64-apple-darwin.tar.gz"
      sha256 "460be8b281860e9cec4bb6eaa0b7a30ac82fa0286df091672c54dc0c09023eb1"
    end
    on_intel do
      odie "Intel Mac binaries are not provided. Run `cargo install --git https://github.com/grahambrooks/colab --locked` to build from source."
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/grahambrooks/colab/releases/download/v2026.7.0/colab-v2026.7.0-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "b39084135336a4c2fd3e85b80b84b2e3666230b97d7278e2e4aec908b0581de0"
    end
    on_intel do
      url "https://github.com/grahambrooks/colab/releases/download/v2026.7.0/colab-v2026.7.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "72a389ea8201e03cdf2f0e683292f4bdca46d9257c0b9813c90f8703c93a5ae1"
    end
  end

  def install
    bin.install "colab"
  end

  test do
    assert_path_exists bin/"colab"
  end
end
