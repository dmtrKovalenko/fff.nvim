class Fff < Formula
  desc "Fast frecency-ranked file finder MCP server for AI code assistants"
  homepage "https://github.com/abhijit-s/fff"
  url "https://github.com/abhijit-s/fff/archive/refs/tags/v0.10.0.tar.gz"
  sha256 "dd6dfe1468aceedd3b3d26d9ce62c17c273b1d1af1be5fa334cbb5b2206eb99c"
  license "MIT"
  # Local dev: brew install --HEAD abhijit-s/fff/fff
  head do
    url "https://github.com/abhijit-s/fff.git", branch: "main"
  end

  depends_on "rust" => :build

  def install
    # Prevent vendored libgit2's cmake build from linking Homebrew's sqlite.
    # libgit2 uses sqlite only for credential caching, which fff does not use.
    ENV["CMAKE_ARGS"] = "-DUSE_SQLITE_CREDENTIAL_CACHING=OFF"

    system "cargo", "build", "--release", "--no-default-features",
           "-p", "fff-engine", "-p", "fff-mcp"

    # fff-mcp locates fff-engine via current_exe().parent() at runtime,
    # so both binaries must be installed to the same directory.
    bin.install "target/release/fff-mcp"
    bin.install "target/release/fff-engine"
  end

  def caveats
    <<~EOS
      fff-mcp and fff-engine are both installed to #{HOMEBREW_PREFIX}/bin/.

      Register with Claude Code (user-scoped, survives updates):
        claude mcp add -s user fff -- #{bin}/fff-mcp

      Or add to your project .mcp.json:
        {
          "mcpServers": {
            "fff": { "type": "stdio", "command": "fff-mcp" }
          }
        }

      Configuration (optional): ~/.config/fff/config.toml
        [log]
        level = "fff_engine=info,fff_mcp=info,warn"
    EOS
  end

  test do
    assert_predicate bin/"fff-mcp", :executable?
    assert_predicate bin/"fff-engine", :executable?
    assert_match "fff-engine", shell_output("#{bin}/fff-engine --help 2>&1", 2)
  end
end
