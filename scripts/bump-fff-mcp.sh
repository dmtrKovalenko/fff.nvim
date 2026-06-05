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
RELEASE_BASE="https://github.com/${REPO}/releases/download"

fetch_sha256() {
  local asset="$1"
  curl -fsSL "${RELEASE_BASE}/v${VERSION}/${asset}.sha256" | awk '{print $1}'
}

sha_darwin_arm="$(fetch_sha256 fff-mcp-aarch64-apple-darwin)"
sha_darwin_intel="$(fetch_sha256 fff-mcp-x86_64-apple-darwin)"
sha_linux_arm="$(fetch_sha256 fff-mcp-aarch64-unknown-linux-gnu)"
sha_linux_intel="$(fetch_sha256 fff-mcp-x86_64-unknown-linux-gnu)"

cat >"$FORMULA_PATH" <<'RUBY'
# Originally authored by @jellydn (https://github.com/jellydn/homebrew-tap).
# Maintained in-repo; auto-bumped by .github/workflows/release.yaml on stable releases.
class FffMcp < Formula
  desc "Fast file search toolkit for AI agents (MCP server)"
  homepage "https://github.com/__REPO__"
  license "MIT"
  version "__VERSION__"

  LIVECHECK_REPO = "__REPO__".freeze
  RELEASE_BASE = "https://github.com/__REPO__/releases/download".freeze

  on_macos do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-apple-darwin"
      sha256 "__SHA_DARWIN_ARM__"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-apple-darwin"
      sha256 "__SHA_DARWIN_INTEL__"
    end
  end

  on_linux do
    on_arm do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-aarch64-unknown-linux-gnu"
      sha256 "__SHA_LINUX_ARM__"
    end

    on_intel do
      url "#{RELEASE_BASE}/v#{version}/fff-mcp-x86_64-unknown-linux-gnu"
      sha256 "__SHA_LINUX_INTEL__"
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
    system bin/"fff-mcp", "--version"
  end
end
RUBY

if [[ "$(uname)" == "Darwin" ]]; then
  sed -i '' \
    -e "s/__VERSION__/${VERSION}/g" \
    -e "s|__REPO__|${REPO}|g" \
    -e "s/__SHA_DARWIN_ARM__/${sha_darwin_arm}/g" \
    -e "s/__SHA_DARWIN_INTEL__/${sha_darwin_intel}/g" \
    -e "s/__SHA_LINUX_ARM__/${sha_linux_arm}/g" \
    -e "s/__SHA_LINUX_INTEL__/${sha_linux_intel}/g" \
    "$FORMULA_PATH"
else
  sed -i \
    -e "s/__VERSION__/${VERSION}/g" \
    -e "s|__REPO__|${REPO}|g" \
    -e "s/__SHA_DARWIN_ARM__/${sha_darwin_arm}/g" \
    -e "s/__SHA_DARWIN_INTEL__/${sha_darwin_intel}/g" \
    -e "s/__SHA_LINUX_ARM__/${sha_linux_arm}/g" \
    -e "s/__SHA_LINUX_INTEL__/${sha_linux_intel}/g" \
    "$FORMULA_PATH"
fi

echo "Wrote ${FORMULA_PATH} for ${TAG}"