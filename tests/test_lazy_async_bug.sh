#!/usr/bin/env bash
# tests/test_lazy_async_bug.sh
#
# Shell-level proof of the async-exit bug in the original download_or_build_binary().
#
# When lazy.nvim's build hook returns, Neovim may exit moments later.
# The old implementation fired vim.system subprocesses and returned immediately,
# so those subprocesses (git → curl → sha → rename) were orphaned on exit and
# the binary was never written to disk.
#
# The fix wraps the whole chain in vim.wait, keeping the event loop alive until
# the rename completes.
#
# Usage: bash tests/test_lazy_async_bug.sh

set -euo pipefail

PLUGIN_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export FFF_PLUGIN_ROOT="$PLUGIN_ROOT"   # picked up by os.getenv() inside Lua

case "$(uname -s)" in
  Darwin)           EXT=dylib ;;
  MINGW*|MSYS*|CYGWIN*) EXT=dll ;;
  *)                EXT=so ;;
esac
BINARY="$PLUGIN_ROOT/target/release/libfff_nvim.$EXT"

passed=0; failed=0
pass()      { printf '  PASS  %s\n'   "$1";      (( passed += 1 )) || true; }
fail()      { printf '  FAIL  %s\n'   "$1" >&2;  (( failed += 1 )) || true; }
assert_file()    { [[ -f $1 ]] && pass "$2" || fail "$2 (file missing: $1)"; }
assert_no_file() { [[ ! -f $1 ]] && pass "$2" || fail "$2 (file unexpectedly present: $1)"; }

# ── Temp runners (cleaned up on exit) ────────────────────────────────────────
RUNNERS=$(mktemp -d)
trap 'rm -rf "$RUNNERS"' EXIT

# Runner A — simulates OLD build hook:
#   ensure_downloaded fires async vim.system calls, then returns.
#   os.exit(0) mimics Neovim exiting immediately after the hook returns —
#   the event loop never spins again, so the git/curl/rename callbacks die.
cat >"$RUNNERS/async.lua" <<'LUA'
vim.opt.runtimepath:prepend(os.getenv('FFF_PLUGIN_ROOT'))
require('fff.download').ensure_downloaded(
  { version = '3e9b865', force = true },
  function() end   -- this callback is never reached
)
os.exit(0)         -- immediate exit, same effect as lazy returning from the hook
LUA

# Runner B — exercises the FIXED download_or_build_binary():
#   vim.wait spins the event loop until the rename lands on disk,
#   so the function only returns after the binary is present.
cat >"$RUNNERS/blocking.lua" <<'LUA'
vim.opt.runtimepath:prepend(os.getenv('FFF_PLUGIN_ROOT'))
-- Blocks via vim.wait; only returns once the binary is on disk.
require('fff.download').download_or_build_binary()
LUA

printf '\n=== lazy.nvim async-exit bug ===\nbinary : %s\n\n' "$BINARY"

# ── Part 1: old async (broken) ────────────────────────────────────────────────
echo '--- Part 1: OLD async behavior — fire subprocesses and exit ---'
rm -f "$BINARY" "$BINARY.tmp"

nvim -l "$RUNNERS/async.lua" 2>/dev/null

# Give any orphaned subprocesses a full second to do whatever they can.
# They finish running (git/curl), but the Neovim rename callback is dead,
# so the binary never moves from .tmp → libfff_nvim.dylib.
sleep 1
assert_no_file "$BINARY" "binary absent — rename callback was killed with the process"

# ── Part 2: fixed blocking ────────────────────────────────────────────────────
printf '\n--- Part 2: FIXED behavior --- vim.wait keeps event loop alive ---\n'
rm -f "$BINARY" "$BINARY.tmp"

nvim -l "$RUNNERS/blocking.lua"   # prints download progress to stdout

assert_file "$BINARY" "binary present — vim.wait held the process until rename succeeded"

# ── Summary ────────────────────────────────────────────────────────────────────
printf '\n%d passed  %d failed\n' "$passed" "$failed"
(( failed == 0 ))
