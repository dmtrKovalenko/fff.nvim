# Originally authored by @jellydn (https://github.com/jellydn/homebrew-tap).
# Maintained in-repo; auto-bumped by .github/workflows/release.yaml on stable releases.
class FffMcp < Formula
  desc "Fast file search toolkit for AI agents (MCP server)"
  homepage "https://github.com/dmtrKovalenko/fff.nvim"
  license "MIT"
  version "0.10.3"

  LIVECHECK_REPO = "dmtrKovalenko/fff.nvim".freeze
  RELEASE_BASE = "https://github.com/dmtrKovalenko/fff.nvim/releases/download".freeze

  on_macos do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-apple-darwin"
      sha256 "8c52acf1e95540f02cf55579126ebf580cf48af15c76e1c90301c2c8c5acbcf9"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-apple-darwin"
      sha256 "f72c31c7f3355d37ec526ba56fd716341bd7186a15a399910cf3f2b46fef2099"
    end
  end

  on_linux do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-unknown-linux-gnu"
      sha256 "ed2db5b7268c76619178594c31e400b0973668acfa42fe6b9a06a28ad459b667"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-unknown-linux-gnu"
      sha256 "2692bc567f8c5609a179795f5ee95555d783a678acdad9c9883e6730654a057e"
    end
  end

  livecheck do
    url "https://github.com/#{LIVECHECK_REPO}/releases/latest"
    strategy :github_latest
  end

  def install
    if OS.mac?
      if Hardware::CPU.arm?
        bin.install "fff-mcp-aarch64-apple-darwin" => "fff-mcp"
      elsif Hardware::CPU.intel?
        bin.install "fff-mcp-x86_64-apple-darwin" => "fff-mcp"
      end
    elsif OS.linux?
      if Hardware::CPU.arm?
        bin.install "fff-mcp-aarch64-unknown-linux-gnu" => "fff-mcp"
      elsif Hardware::CPU.intel?
        bin.install "fff-mcp-x86_64-unknown-linux-gnu" => "fff-mcp"
      end
    end
  end

  test do
    system bin/"fff-mcp", "--healthcheck"
  end
end
