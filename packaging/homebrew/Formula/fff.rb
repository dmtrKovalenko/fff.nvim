class Fff < Formula
  desc "Fast frecency-ranked file finder MCP server for AI code assistants"
  homepage "https://github.com/abhijit-s/fff"
  url "https://github.com/abhijit-s/fff/archive/refs/tags/v0.11.0.tar.gz"
  sha256 "af4acaf179982f30ef89b7709f3ed8419357457dab304d5c80d7942bbaf18d69"
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
           "-p", "fff-engine", "-p", "fff-mcp", "-p", "fff-ctl"

    # fff-mcp locates fff-engine via current_exe().parent() at runtime,
    # so both binaries must be installed to the same directory.
    bin.install "target/release/fff-mcp"
    bin.install "target/release/fff-engine"
    bin.install "target/release/fffctl"
  end

  def caveats
    <<~EOS
      fff-mcp, fff-engine, and fffctl are all installed to #{HOMEBREW_PREFIX}/bin/.

      Register with Claude Code (user-scoped, survives updates):
        claude mcp add -s user fff -- #{bin}/fff-mcp

      Or add to your project .mcp.json:
        {
          "mcpServers": {
            "fff": { "type": "stdio", "command": "fff-mcp" }
          }
        }

      Manage running daemons with fffctl:
        fffctl list           # show all running daemons
        fffctl stop --all     # stop every daemon
        fffctl clean          # remove stale lockfiles / orphan sockets

      Configuration (optional): ~/.config/fff/config.toml
        [log]
        level = "fff_engine=info,fff_mcp=info,warn"
    EOS
  end

  test do
    assert_predicate bin/"fff-mcp", :executable?
    assert_predicate bin/"fff-engine", :executable?
    assert_predicate bin/"fffctl", :executable?
    assert_match "fff-engine", shell_output("#{bin}/fff-engine --help 2>&1", 2)
    assert_match "Manage fff-engine daemons", shell_output("#{bin}/fffctl --help 2>&1")
  end
end
