#!/usr/bin/env bash
# Regenerate Formula/fff-mcp.rb from a dmtrKovalenko/fff.nvim GitHub release.
set -euo pipefail

REPO="${FFF_RELEASE_REPO:-dmtrKovalenko/fff.nvim}"
FORMULA_PATH="${FFF_FORMULA_PATH:-Formula/fff-mcp.rb}"

usage() {
  echo "Usage: $0 <version>   # e.g. 0.9.1 (without v prefix)" >&2
  exit 1
}

[[ $# -eq 1 ]] || usage
VERSION="${1#v}"
TAG="v${VERSION}"
BASE="https://github.com/${REPO}/releases/download/${TAG}"

fetch_sha256() {
  local asset="$1"
  curl -fsSL "${BASE}/${asset}.sha256" | awk '{print $1}'
}

sha_darwin_arm="$(fetch_sha256 fff-mcp-aarch64-apple-darwin)"
sha_darwin_intel="$(fetch_sha256 fff-mcp-x86_64-apple-darwin)"
sha_linux_arm="$(fetch_sha256 fff-mcp-aarch64-unknown-linux-gnu)"
sha_linux_intel="$(fetch_sha256 fff-mcp-x86_64-unknown-linux-gnu)"

cat >"$FORMULA_PATH" <<EOF
# Originally authored by @jellydn (https://github.com/jellydn/homebrew-tap).
# Maintained in-repo; auto-bumped by .github/workflows/release.yaml on stable releases.
class FffMcp < Formula
  desc "Fast file search toolkit for AI agents (MCP server)"
  homepage "https://github.com/${REPO}"
  license "MIT"

  # Homebrew reserves \`version\`; use a local for the release tag.
  v = "${VERSION}"

  on_macos do
    on_arm do
      url "${BASE}/fff-mcp-aarch64-apple-darwin"
      sha256 "${sha_darwin_arm}"
    end

    on_intel do
      url "${BASE}/fff-mcp-x86_64-apple-darwin"
      sha256 "${sha_darwin_intel}"
    end
  end

  on_linux do
    on_arm do
      url "${BASE}/fff-mcp-aarch64-unknown-linux-gnu"
      sha256 "${sha_linux_arm}"
    end

    on_intel do
      url "${BASE}/fff-mcp-x86_64-unknown-linux-gnu"
      sha256 "${sha_linux_intel}"
    end
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
    system bin/"fff-mcp", "--version"
  end
end
EOF

echo "Wrote ${FORMULA_PATH} for ${TAG}"