# abhijit-s/fff Homebrew tap

Installs `fff-mcp` and `fff-engine` — the fast frecency-ranked file finder
MCP server for AI code assistants.

## Usage

**Local tap (dev / pre-release):**

```bash
brew tap abhijit-s/fff /path/to/fff/packaging/homebrew
brew install abhijit-s/fff/fff
```

**Published tap (once a GitHub release exists):**

```bash
brew tap abhijit-s/fff https://github.com/abhijit-s/homebrew-fff
brew install abhijit-s/fff/fff
```

**HEAD build (from main branch):**

```bash
brew install --HEAD abhijit-s/fff/fff
```

## After install

Register with Claude Code:

```bash
claude mcp add -s user fff -- $(brew --prefix)/bin/fff-mcp
```
