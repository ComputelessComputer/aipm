class Aipm < Formula
  desc "AI-powered project manager for the terminal"
  homepage "https://github.com/ComputelessComputer/aipm"
  url "https://github.com/ComputelessComputer/aipm/archive/refs/tags/v0.3.1.tar.gz"
  sha256 "4ac43ff6f2b6f68bf4e766bc9b46d1ddda728a70fd06a1dd9e027091675ea816"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/aipm --version")
  end
end
