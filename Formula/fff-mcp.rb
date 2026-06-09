# Originally authored by @jellydn (https://github.com/jellydn/homebrew-tap).
# Maintained in-repo; auto-bumped by .github/workflows/release.yaml on stable releases.
class FffMcp < Formula
  desc "Fast file search toolkit for AI agents (MCP server)"
  homepage "https://github.com/dmtrKovalenko/fff.nvim"
  license "MIT"
  version "0.9.1"

  LIVECHECK_REPO = "dmtrKovalenko/fff.nvim".freeze
  RELEASE_BASE = "https://github.com/dmtrKovalenko/fff.nvim/releases/download".freeze

  on_macos do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-apple-darwin"
      sha256 "cee75793fff67b25b95303c4708ec88b2c707f5ca51df5975a77330fc169f6ab"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-apple-darwin"
      sha256 "37b7d09098c5a1a0f036111e125885f58f3e3ec65d5eac7dd1f5f773407dd0c9"
    end
  end

  on_linux do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-unknown-linux-gnu"
      sha256 "32cae6f55e0eed764d0ae150c2e6c9332608b131442b56a14d9afa9877294ef7"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-unknown-linux-gnu"
      sha256 "df96f1a25112d83117909b215e5e4a98917ac885163b23147c1819c4700a4f21"
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
