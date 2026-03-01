#!/usr/bin/env bash
#
# Benchmark: fff MCP vs Claude Code native tools on real search tasks
#
# Usage:
#   ./scripts/benchmark-claude.sh [concept_number] [--fff-only | --native-only]
#
# Runs real Claude Code instances against ~/dev/lightsource:
#   - With fff MCP tools (frecency-ranked, fuzzy search)
#   - With native tools only (Glob, Grep, Read)
# Then compares: tokens, cost, turns, and whether the right file was found.
#
# Requirements:
#   - claude CLI in PATH
#   - ~/dev/lightsource exists
#   - fff MCP server buildable (bun + Rust lib in packages/fff-bun/bin/)
#
# Auth: The script inherits YOUR shell environment. If you use AWS Bedrock,
# make sure your AWS credentials are exported before running.
# Run `claude --print -p "hello"` first to verify auth works.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
LIGHTSOURCE="$HOME/dev/lightsource"
RESULTS_DIR="$SCRIPT_DIR/benchmark-results"
MAX_TURNS=10
TIMEOUT_SEC=300  # 5 min per concept per mode
MODEL="us.anthropic.claude-opus-4-6-v1"

mkdir -p "$RESULTS_DIR"

# Write MCP config to temp file to avoid shell quoting issues.
# Both modes (fff and native) connect the fff MCP so context overhead is identical.
FFF_MCP_FILE=$(mktemp)
trap "rm -f $FFF_MCP_FILE" EXIT

cat > "$FFF_MCP_FILE" <<EOF
{"mcpServers":{"fff":{"type":"stdio","command":"bun","args":["$PROJECT_ROOT/packages/fff-mcp/src/index.ts"]}}}
EOF

# ─── PREFLIGHT CHECK ──────────────────────────────────────────────────────────

echo "Preflight check..."
if ! command -v claude &>/dev/null; then
  echo "ERROR: claude CLI not found in PATH"
  exit 1
fi

if [[ ! -d "$LIGHTSOURCE" ]]; then
  echo "ERROR: $LIGHTSOURCE does not exist"
  exit 1
fi

# Quick auth test — must clear nesting env vars, cd to lightsource, use </dev/null
AUTH_TEST=$(cd "$LIGHTSOURCE" && env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT \
  timeout 60s claude --print --output-format json -p "say ok" --max-turns 1 \
  --mcp-config "$FFF_MCP_FILE" --strict-mcp-config </dev/null 2>&1 || true)
if [[ -z "$AUTH_TEST" ]] || echo "$AUTH_TEST" | grep -q '"is_error":true' 2>/dev/null; then
  echo "ERROR: Claude auth failed. Test output:"
  echo "$AUTH_TEST" | head -5
  echo ""
  echo "If using AWS Bedrock, make sure your AWS credentials are exported:"
  echo "  export AWS_ACCESS_KEY_ID=..."
  echo "  export AWS_SECRET_ACCESS_KEY=..."
  echo "  export AWS_SESSION_TOKEN=..."
  echo ""
  echo "Or run: aws sso login"
  exit 1
fi
echo "  Auth OK"
echo ""

# ─── 10 SEARCH CONCEPTS ───────────────────────────────────────────────────────

declare -a PROMPTS
declare -a TARGETS
declare -a NAMES

NAMES[1]="fuzzy-function-search"
PROMPTS[1]="Find the function that loads metadata for an InProgressQuote in the lightsource codebase. Show me the function signature and which file it's in."
TARGETS[1]="quotes/storage/db/src/model/quote.rs"

NAMES[2]="api-endpoint-discovery"
PROMPTS[2]="Find the GraphQL mutation that handles user file uploads (the prepare upload step). Show me the function and its file path."
TARGETS[2]="user_files_service/graphql/src/mutation.rs"

NAMES[3]="cross-service-config"
PROMPTS[3]="Find where QuotesServiceClient is defined as a struct and how it's constructed. Show me the struct definition and its file."
TARGETS[3]="quotes_service_client"

NAMES[4]="test-file-discovery"
PROMPTS[4]="Find the test file for virtual expression manifests in the quotes engine. Show me the file path and list what tests are in it."
TARGETS[4]="virtual_expression_manifest_test"

NAMES[5]="error-type-definition"
PROMPTS[5]="Find where the custom Error type with variants like not_found and permission_denied is defined in the common/error crate. Show me the enum or struct definition."
TARGETS[5]="common/error"

NAMES[6]="database-model-search"
PROMPTS[6]="Find the Diesel ORM model struct for InProgressQuote — the actual struct definition with its derives, not usages. Show me the struct and its file path."
TARGETS[6]="quotes/storage/db/src/model/quote.rs"

NAMES[7]="auth-flow-tracing"
PROMPTS[7]="Find where ActorAuth is defined and trace how it's used in service GraphQL contexts. Show me the definition and one example of it being extracted in a resolver."
TARGETS[7]="actor_auth"

NAMES[8]="todo-tech-debt"
PROMPTS[8]="Find TODO comments tagged with github issues numbers (like #... or similar patterns) in the quotes-related code. Show me a few examples with their file paths."
TARGETS[8]="TODO"

NAMES[9]="cross-language-pattern"
PROMPTS[9]="Find code related to QuoteBuilder across both Rust backend and TypeScript frontend. Show me one example from each language."
TARGETS[9]="QuoteBuilder"

NAMES[10]="broad-pattern-search"
PROMPTS[10]="Find the main GraphQL query resolvers for sourcing projects — specifically the resolver that loads a single sourcing project by ID. Show me the resolver function and file."
TARGETS[10]="sourcing_project"

NAMES[11]="file-by-name-lookup"
PROMPTS[11]="What files exist in this repository related to 'quote_builder'? List 10 paths from frontend and 10 from backend."
TARGETS[11]="quote_builder"

# NOTE: Concept 11 tests file-by-name lookup. The model strongly prefers native Glob
# over find_files due to Claude Code's system prompt. find_files would be faster here
# (fuzzy: 'tsconfig sourcing' → 1 call) but the model won't use it unprompted.

# ─── HELPER FUNCTIONS ──────────────────────────────────────────────────────────

millis() {
  python3 -c 'import time; print(int(time.time()*1000))'
}

run_claude() {
  local mode="$1"  # "fff" or "native"
  local concept="$2"
  local raw_prompt="${PROMPTS[$concept]}"
  local outfile="$RESULTS_DIR/${NAMES[$concept]}-${mode}.json"

  # Both modes connect fff MCP so context overhead is identical.
  # The prompt prefix steers which tools Claude actually uses.
  local mcp_args=(--mcp-config "$FFF_MCP_FILE" --strict-mcp-config)
  # (tool_args removed — both modes use identical MCP config, prompt steers tool choice)

  local prompt
  if [[ "$mode" == "fff" ]]; then
    prompt="Use fff tools (grep, find_files, multi_grep) instead of native Glob/Grep.

$raw_prompt"
  else
    prompt="IMPORTANT: For file search and content search, use ONLY the native tools (Glob, Grep, Read). Do NOT use any mcp__fff__* tools. Ignore the fff MCP server entirely.

$raw_prompt"
  fi

  local model_args=()
  if [[ -n "$MODEL" ]]; then
    model_args=(--model "$MODEL")
  fi

  local errfile="$RESULTS_DIR/${NAMES[$concept]}-${mode}.stderr"
  local streamfile="$RESULTS_DIR/${NAMES[$concept]}-${mode}.stream.jsonl"

  echo "  Running [$mode] concept $concept: ${NAMES[$concept]} (timeout ${TIMEOUT_SEC}s)..."

  local start_time
  start_time=$(millis)

  # Capture stream-json for per-turn analysis, then extract the final result.
  # IMPORTANT: </dev/null prevents stdin blocking, cd to LIGHTSOURCE so Claude's
  # tools work in the right directory.
  (
    cd "$LIGHTSOURCE"
    timeout "${TIMEOUT_SEC}s" env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT \
      claude \
      --print \
      --verbose \
      --output-format stream-json \
      --max-turns "$MAX_TURNS" \
      --max-budget-usd 0.50 \
      --dangerously-skip-permissions \
      "${model_args[@]}" \
      "${mcp_args[@]}" \
      -p "$prompt" \
      </dev/null \
      > "$streamfile" 2>"$errfile"
  ) || {
    local exit_code=$?
    if [[ $exit_code -eq 124 ]]; then
      echo "    TIMEOUT after ${TIMEOUT_SEC}s"
      echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"TIMEOUT after ${TIMEOUT_SEC}s\",\"num_turns\":0,\"total_cost_usd\":0,\"duration_ms\":0,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}" > "$outfile"
      return
    elif [[ ! -s "$streamfile" ]]; then
      echo "    FAILED (exit $exit_code)"
      local stderr_msg
      stderr_msg=$(head -3 "$errfile" 2>/dev/null | tr '\n' ' ')
      echo "{\"type\":\"result\",\"is_error\":true,\"result\":\"Process failed (exit $exit_code): $stderr_msg\",\"num_turns\":0,\"total_cost_usd\":0,\"duration_ms\":0,\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}" > "$outfile"
    fi
  }

  # Print stderr if non-empty (helps debugging)
  if [[ -s "$errfile" ]]; then
    echo "    stderr: $(head -1 "$errfile")"
  fi

  # Extract final result JSON from stream (last line with type=result)
  if [[ -s "$streamfile" ]]; then
    grep '"type":"result"' "$streamfile" | tail -1 > "$outfile" 2>/dev/null || true
  fi

  local end_time
  end_time=$(millis)
  local wall_ms=$(( end_time - start_time ))

  # Inject wall time into the JSON
  if [[ -f "$outfile" ]] && [[ -s "$outfile" ]]; then
    local tmp
    tmp=$(mktemp)
    jq --argjson wall "$wall_ms" '. + {wall_ms: $wall}' "$outfile" > "$tmp" 2>/dev/null && mv "$tmp" "$outfile" || rm -f "$tmp"
  fi

  # Quick status line
  local cost turns found_str cost_fmt
  cost=$(jq -r '.total_cost_usd // 0' "$outfile" 2>/dev/null || echo "0")
  cost_fmt=$(printf '%.4f' "$cost" 2>/dev/null || echo "$cost")
  turns=$(jq -r '.num_turns // 0' "$outfile" 2>/dev/null || echo "?")
  local err=$(jq -r '.is_error // false' "$outfile" 2>/dev/null || echo "?")
  if [[ "$err" == "true" ]]; then
    found_str="ERROR"
  else
    found_str="ok"
  fi
  echo "    Done in $((wall_ms/1000))s | \$$cost_fmt | ${turns} turns | $found_str"

  # ── Per-turn tool call analysis ──
  if [[ -s "$streamfile" ]]; then
    echo ""
    echo "    ┌─ Tool call trace [$mode] ─────────────────────────────────────────"
    # Extract tool_use events and tool_result sizes from stream
    python3 - "$streamfile" <<'PYEOF'
import json, sys

stream_file = sys.argv[1]
tool_calls = []       # (name, summary, tool_use_id)
result_sizes = {}     # tool_use_id -> content_length

with open(stream_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except:
            continue

        msg_type = msg.get("type", "")
        content_blocks = []

        if msg_type == "assistant" and "message" in msg:
            content_blocks = msg["message"].get("content", [])
        elif msg_type == "user" and "message" in msg:
            content_blocks = msg["message"].get("content", [])

        for block in content_blocks:
            if not isinstance(block, dict):
                continue

            # Tool use (Claude calling a tool)
            if block.get("type") == "tool_use":
                name = block.get("name", "?")
                inp = block.get("input", {})
                if "query" in inp:
                    summary = f'query="{inp["query"]}"'
                elif "pattern" in inp:
                    summary = f'pattern="{inp["pattern"]}"'
                elif "patterns" in inp:
                    summary = f'patterns={json.dumps(inp["patterns"])}'
                elif "file_path" in inp:
                    summary = f'file="{inp["file_path"][-60:]}"'
                elif "path" in inp:
                    summary = f'path="{inp["path"][-60:]}"'
                elif "command" in inp:
                    summary = f'cmd="{inp["command"][:70]}"'
                else:
                    summary = str(inp)[:70]
                tool_calls.append((name, summary, block.get("id", "")))

            # Tool result (response back from tool)
            if block.get("type") == "tool_result":
                tid = block.get("tool_use_id", "")
                content = block.get("content", "")
                if isinstance(content, list):
                    total_len = sum(len(c.get("text", "")) for c in content if isinstance(c, dict))
                elif isinstance(content, str):
                    total_len = len(content)
                else:
                    total_len = len(str(content))
                result_sizes[tid] = total_len

# Print trace
for i, (name, summary, tid) in enumerate(tool_calls, 1):
    rsize = result_sizes.get(tid, -1)
    size_str = f" -> {rsize:,} chars" if rsize >= 0 else ""
    print(f"    |  {i:2d}. {name:25s} {summary[:50]:50s}{size_str}")

if not tool_calls:
    print("    |  (no tool calls captured)")

print(f"    | Total: {len(tool_calls)} tool calls")
PYEOF
    echo "    └──────────────────────────────────────────────────────────────────"
    echo ""
  fi
}

parse_result() {
  local jsonfile="$1"
  local target="$2"

  if [[ ! -f "$jsonfile" ]] || [[ ! -s "$jsonfile" ]]; then
    echo "0|0|0|0|false|false"
    return
  fi

  local cost_usd num_turns duration_ms wall_ms is_error result
  cost_usd=$(jq -r '.total_cost_usd // 0' "$jsonfile" 2>/dev/null || echo "0")
  num_turns=$(jq -r '.num_turns // 0' "$jsonfile" 2>/dev/null || echo "0")
  duration_ms=$(jq -r '.duration_ms // 0' "$jsonfile" 2>/dev/null || echo "0")
  wall_ms=$(jq -r '.wall_ms // 0' "$jsonfile" 2>/dev/null || echo "0")
  is_error=$(jq -r '.is_error // false' "$jsonfile" 2>/dev/null || echo "false")
  result=$(jq -r '.result // ""' "$jsonfile" 2>/dev/null || echo "")

  local found="false"
  if echo "$result" | grep -qi "$target" 2>/dev/null; then
    found="true"
  fi

  echo "${cost_usd}|${num_turns}|${duration_ms}|${wall_ms}|${is_error}|${found}"
}

print_comparison() {
  local concept="$1"
  local name="${NAMES[$concept]}"
  local target="${TARGETS[$concept]}"

  local fff_file="$RESULTS_DIR/${name}-fff.json"
  local native_file="$RESULTS_DIR/${name}-native.json"

  local fff_data native_data
  fff_data=$(parse_result "$fff_file" "$target")
  native_data=$(parse_result "$native_file" "$target")

  IFS='|' read -r fff_cost fff_turns fff_dur fff_wall fff_err fff_found <<< "$fff_data"
  IFS='|' read -r nat_cost nat_turns nat_dur nat_wall nat_err nat_found <<< "$native_data"

  # Token counts from usage
  local fff_input fff_output nat_input nat_output
  fff_input=$(jq -r '.usage.input_tokens // 0' "$fff_file" 2>/dev/null || echo "0")
  fff_output=$(jq -r '.usage.output_tokens // 0' "$fff_file" 2>/dev/null || echo "0")
  nat_input=$(jq -r '.usage.input_tokens // 0' "$native_file" 2>/dev/null || echo "0")
  nat_output=$(jq -r '.usage.output_tokens // 0' "$native_file" 2>/dev/null || echo "0")
  local fff_tokens=$((fff_input + fff_output))
  local nat_tokens=$((nat_input + nat_output))

  # Determine winner
  local winner="tie"
  if [[ "$fff_found" == "true" && "$nat_found" == "false" ]]; then
    winner="FFF"
  elif [[ "$fff_found" == "false" && "$nat_found" == "true" ]]; then
    winner="NATIVE"
  elif [[ "$fff_found" == "true" && "$nat_found" == "true" ]]; then
    # Both found — compare cost with 15% tolerance band for ties
    local ratio
    ratio=$(echo "scale=4; $fff_cost / $nat_cost" | bc 2>/dev/null || echo "1")
    # ratio < 0.85 means FFF is >15% cheaper → FFF wins
    # ratio > 1.15 means FFF is >15% more expensive → NATIVE wins
    # otherwise → tie
    local ratio_x100
    ratio_x100=$(echo "$ratio * 100" | bc 2>/dev/null | cut -d. -f1 || echo "100")
    if [[ "${ratio_x100:-100}" -lt 85 ]]; then
      winner="FFF"
    elif [[ "${ratio_x100:-100}" -gt 115 ]]; then
      winner="NATIVE"
    fi
  fi

  # Format costs to 4 decimal places
  local fff_cost_fmt nat_cost_fmt
  fff_cost_fmt=$(printf '%.4f' "$fff_cost" 2>/dev/null || echo "$fff_cost")
  nat_cost_fmt=$(printf '%.4f' "$nat_cost" 2>/dev/null || echo "$nat_cost")

  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo " CONCEPT $concept: $name"
  echo " Target: $target"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  printf "  %-12s │ %10s │ %6s │ %8s │ %8s │ %7s │ %7s\n" "" "Cost" "Turns" "Tokens" "Wall (s)" "Found?" "Error?"
  echo "  ─────────────┼────────────┼────────┼──────────┼──────────┼─────────┼────────"
  printf "  %-12s │ %10s │ %6s │ %8s │ %8s │ %7s │ %7s\n" \
    "fff MCP" "\$$fff_cost_fmt" "$fff_turns" "$fff_tokens" "$((fff_wall/1000))" "$fff_found" "$fff_err"
  printf "  %-12s │ %10s │ %6s │ %8s │ %8s │ %7s │ %7s\n" \
    "Native" "\$$nat_cost_fmt" "$nat_turns" "$nat_tokens" "$((nat_wall/1000))" "$nat_found" "$nat_err"
  echo "  ─────────────┴────────────┴────────┴──────────┴──────────┴─────────┴────────"

  # Cost savings percentage
  if [[ "$nat_cost" != "0" ]]; then
    local cost_savings
    cost_savings=$(echo "scale=1; (1 - $fff_cost / $nat_cost) * 100" | bc 2>/dev/null || echo "?")
    echo "  Cost savings: ${cost_savings}% (fff: \$$fff_cost_fmt, native: \$$nat_cost_fmt)"
  fi

  echo "  WINNER: $winner"
  echo ""
}

# ─── MAIN ──────────────────────────────────────────────────────────────────────

SELECTED=""
MODE="both"  # both, fff-only, native-only

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fff-only)    MODE="fff"; shift ;;
    --native-only) MODE="native"; shift ;;
    --model)       MODEL="$2"; shift 2 ;;
    --max-turns)   MAX_TURNS="$2"; shift 2 ;;
    --timeout)     TIMEOUT_SEC="$2"; shift 2 ;;
    [0-9]*)        SELECTED="$1"; shift ;;
    *)
      echo "Usage: $0 [1-10] [options]"
      echo ""
      echo "Options:"
      echo "  --fff-only       Only run fff MCP (skip native)"
      echo "  --native-only    Only run native tools (skip fff)"
      echo "  --model MODEL    Use specific model (e.g., haiku, sonnet)"
      echo "  --max-turns N    Max agentic turns per run (default: 10)"
      echo "  --timeout SEC    Timeout per run in seconds (default: 300)"
      exit 1
      ;;
  esac
done

echo "╔════════════════════════════════════════════════════════════════════════════╗"
echo "   Target: ~/dev/lightsource (194K files)                                     "
echo "   Max turns: $MAX_TURNS | Timeout: ${TIMEOUT_SEC}s | Budget: \$0.50/run"
echo "╚════════════════════════════════════════════════════════════════════════════╝"
echo ""

if [[ -n "$SELECTED" ]]; then
  concepts=("$SELECTED")
else
  concepts=(1 2 3 4 5 6 7 8 9 10 11)
fi

for c in "${concepts[@]}"; do
  echo "── Concept $c: ${NAMES[$c]} ──"

  if [[ "$MODE" == "both" || "$MODE" == "fff" ]]; then
    run_claude "fff" "$c"
  fi

  if [[ "$MODE" == "both" || "$MODE" == "native" ]]; then
    run_claude "native" "$c"
  fi

  if [[ "$MODE" == "both" ]]; then
    print_comparison "$c"
  fi
done

# ─── FINAL ANALYSIS ──────────────────────────────────────────────────────────

if [[ "$MODE" == "both" && ${#concepts[@]} -ge 3 ]]; then
  echo ""
  echo "╔════════════════════════════════════════════════════════════════════════════╗"
  echo "   ANALYSIS                                                                  "
  echo "╚════════════════════════════════════════════════════════════════════════════╝"

  python3 - "$RESULTS_DIR" "${concepts[*]}" <<'ANALYSIS_EOF'
import json, os, sys
from pathlib import Path

results_dir = sys.argv[1]
concepts = [int(x) for x in sys.argv[2].split()]

NAMES = {
    1: "fuzzy-function-search",
    2: "api-endpoint-discovery",
    3: "cross-service-config",
    4: "test-file-discovery",
    5: "error-type-definition",
    6: "database-model-search",
    7: "auth-flow-tracing",
    8: "todo-tech-debt",
    9: "cross-language-pattern",
    10: "broad-pattern-search",
    11: "file-by-name-lookup",
}

def load_result(name, mode):
    path = os.path.join(results_dir, f"{name}-{mode}.json")
    if not os.path.exists(path):
        return None
    try:
        with open(path) as f:
            return json.load(f)
    except:
        return None

def load_traces(name, mode):
    """Extract tool calls with their input and result sizes from stream file."""
    path = os.path.join(results_dir, f"{name}-{mode}.stream.jsonl")
    if not os.path.exists(path):
        return []

    tool_calls = []
    result_sizes = {}

    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except:
                continue

            content_blocks = []
            msg_type = msg.get("type", "")
            if msg_type == "assistant" and "message" in msg:
                content_blocks = msg["message"].get("content", [])
            elif msg_type == "user" and "message" in msg:
                content_blocks = msg["message"].get("content", [])

            for block in content_blocks:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "tool_use":
                    inp = block.get("input", {})
                    tool_calls.append({
                        "name": block.get("name", "?"),
                        "id": block.get("id", ""),
                        "input": inp,
                        "query": inp.get("query", inp.get("pattern", inp.get("patterns", inp.get("file_path", "")))),
                    })
                if block.get("type") == "tool_result":
                    tid = block.get("tool_use_id", "")
                    content = block.get("content", "")
                    if isinstance(content, list):
                        total = sum(len(c.get("text", "")) for c in content if isinstance(c, dict))
                    elif isinstance(content, str):
                        total = len(content)
                    else:
                        total = len(str(content))
                    result_sizes[tid] = total

    for tc in tool_calls:
        tc["result_chars"] = result_sizes.get(tc["id"], -1)

    return tool_calls

# ── Collect all data ──
rows = []
total_fff = 0
total_nat = 0
fff_wins = 0
nat_wins = 0
ties = 0

for c in concepts:
    name = NAMES.get(c, f"concept-{c}")
    fff = load_result(name, "fff")
    nat = load_result(name, "native")
    if not fff or not nat:
        continue

    fc = fff.get("total_cost_usd", 0)
    nc = nat.get("total_cost_usd", 0)
    ft = fff.get("num_turns", 0)
    nt = nat.get("num_turns", 0)
    fw = fff.get("wall_ms", 0) / 1000
    nw = nat.get("wall_ms", 0) / 1000

    total_fff += fc
    total_nat += nc

    if nc > 0:
        ratio = fc / nc
    else:
        ratio = 1.0

    if ratio < 0.85:
        winner = "FFF"
        fff_wins += 1
    elif ratio > 1.15:
        winner = "NATIVE"
        nat_wins += 1
    else:
        winner = "TIE"
        ties += 1

    fff_traces = load_traces(name, "fff")
    nat_traces = load_traces(name, "native")

    rows.append({
        "num": c, "name": name,
        "fff_cost": fc, "nat_cost": nc,
        "fff_turns": ft, "nat_turns": nt,
        "fff_wall": fw, "nat_wall": nw,
        "winner": winner, "ratio": ratio,
        "fff_traces": fff_traces, "nat_traces": nat_traces,
    })

# ── Summary table ──
print()
print(f"  {'#':>2} {'Concept':<28} {'FFF $':>8} {'Nat $':>8} {'Δ':>6} {'FFF T':>5} {'Nat T':>5} {'Winner':>8}")
print(f"  {'─'*2} {'─'*28} {'─'*8} {'─'*8} {'─'*6} {'─'*5} {'─'*5} {'─'*8}")

for r in rows:
    savings = (1 - r["ratio"]) * 100
    print(f"  {r['num']:>2} {r['name']:<28} ${r['fff_cost']:.4f} ${r['nat_cost']:.4f} {savings:>+5.0f}% {r['fff_turns']:>5} {r['nat_turns']:>5} {r['winner']:>8}")

if total_nat > 0:
    overall = (1 - total_fff / total_nat) * 100
else:
    overall = 0

print()
print(f"  Score: FFF {fff_wins} | Native {nat_wins} | Tie {ties}")
print(f"  Total: FFF ${total_fff:.4f} | Native ${total_nat:.4f} | Savings: {overall:+.1f}%")
print()

# ── Waste pattern analysis ──
print("  ┌─ WASTE ANALYSIS ────────────────────────────────────────────────────")

for r in rows:
    traces = r["fff_traces"]
    if not traces:
        continue

    issues = []

    # Count tool types
    tool_search_calls = [t for t in traces if t["name"] == "ToolSearch"]
    read_calls = [t for t in traces if t["name"] == "Read"]
    grep_calls = [t for t in traces if "grep" in t["name"].lower()]
    find_calls = [t for t in traces if "find" in t["name"].lower()]

    # Issue: ToolSearch overhead (each costs ~a turn)
    if len(tool_search_calls) >= 2:
        issues.append(f"{len(tool_search_calls)} ToolSearch calls (model loading tools in multiple turns)")

    # Issue: Read after grep (grep didn't give enough context)
    if read_calls and grep_calls:
        read_files = set()
        for rc in read_calls:
            fp = rc.get("input", {}).get("file_path", "")
            if fp:
                read_files.add(fp.split("/")[-1])
        issues.append(f"Read calls after grep ({', '.join(read_files)}) — grep output wasn't sufficient")

    # Issue: Many grep calls with tiny results (model is probing)
    tiny_greps = [t for t in grep_calls if 0 <= t["result_chars"] <= 50]
    if len(tiny_greps) >= 2:
        queries = [str(t["query"])[:40] for t in tiny_greps]
        issues.append(f"{len(tiny_greps)} greps returned ≤50 chars: {queries}")

    # Issue: Large result from Read (could have been avoided)
    for rc in read_calls:
        if rc["result_chars"] > 5000:
            fn = rc.get("input", {}).get("file_path", "?").split("/")[-1]
            issues.append(f"Read({fn}) returned {rc['result_chars']:,} chars — expensive")

    # Issue: grep returned huge result (over-broad query)
    for gc in grep_calls:
        if gc["result_chars"] > 3000:
            q = str(gc["query"])[:40]
            issues.append(f"Grep({q}) returned {gc['result_chars']:,} chars — too broad")

    # Issue: Sequential greps that could have been multi_grep
    if len(grep_calls) >= 3 and not any("multi" in t["name"].lower() for t in traces):
        issues.append(f"{len(grep_calls)} sequential greps — could multi_grep reduce to 1 call?")

    if issues and r["winner"] != "FFF":
        print(f"  │")
        savings = (1 - r["ratio"]) * 100
        print(f"  │ #{r['num']} {r['name']} ({r['winner']}, {savings:+.0f}%)")
        for issue in issues:
            print(f"  │   • {issue}")

        # Show fff trace summary
        trace_summary = " → ".join(
            t["name"].replace("mcp__fff__", "").replace("ToolSearch", "🔍")
            for t in traces
        )
        print(f"  │   trace: {trace_summary}")

        # Show native trace for comparison
        nat_traces = r["nat_traces"]
        if nat_traces:
            nat_summary = " → ".join(
                t["name"].replace("ToolSearch", "🔍")
                for t in nat_traces
            )
            print(f"  │   native: {nat_summary}")

print("  │")
print("  └────────────────────────────────────────────────────────────────────")
print()

# ── Actionable suggestions ──
print("  ┌─ SUGGESTED IMPROVEMENTS ────────────────────────────────────────────")

# Aggregate patterns across all concepts
total_tool_search = sum(len([t for t in r["fff_traces"] if t["name"] == "ToolSearch"]) for r in rows)
total_reads_after_grep = sum(
    1 for r in rows
    if any("grep" in t["name"].lower() for t in r["fff_traces"])
    and any(t["name"] == "Read" for t in r["fff_traces"])
)
total_tiny_greps = sum(
    len([t for t in r["fff_traces"] if "grep" in t["name"].lower() and 0 <= t["result_chars"] <= 50])
    for r in rows
)
total_sequential_greps = sum(
    len([t for t in r["fff_traces"] if "grep" in t["name"].lower()])
    for r in rows
    if len([t for t in r["fff_traces"] if "grep" in t["name"].lower()]) >= 3
)

if total_reads_after_grep >= 3:
    print(f"  │ 1. EXPAND GREP CONTEXT: {total_reads_after_grep}/{len(rows)} concepts do Read after grep.")
    print(f"  │    Grep results need more inline context to avoid follow-up Reads.")
    print(f"  │    → Increase MAX_DEF_EXPAND, show more body for non-def matches")
    print(f"  │")

if total_tiny_greps >= 5:
    print(f"  │ 2. IMPROVE ZERO/LOW-RESULT GUIDANCE: {total_tiny_greps} greps returned ≤50 chars.")
    print(f"  │    When results are sparse, show related files/symbols to help the model.")
    print(f"  │    → Add 'did you mean?' suggestions or sibling files in same directory")
    print(f"  │")

if total_tool_search >= len(rows) * 1.5:
    print(f"  │ 3. REDUCE TOOLSEARCH OVERHEAD: {total_tool_search} ToolSearch calls across {len(rows)} concepts.")
    print(f"  │    Each ToolSearch costs a turn. Model loads tools incrementally.")
    print(f"  │    → Can't fix directly, but reducing total calls makes this less impactful")
    print(f"  │")

if total_sequential_greps >= 8:
    print(f"  │ 4. PROMOTE MULTI_GREP: {total_sequential_greps} sequential grep calls could be batched.")
    print(f"  │    Model uses sequential grep when multi_grep would be more efficient.")
    print(f"  │    → Improve multi_grep description or auto-suggest in 0-result messages")
    print(f"  │")

# Concept-specific suggestions
losing = [r for r in rows if r["winner"] == "NATIVE"]
if losing:
    print(f"  │ LOSING CONCEPTS ({len(losing)}):")
    for r in losing:
        traces = r["fff_traces"]
        nat_traces = r["nat_traces"]
        fff_grep_count = len([t for t in traces if "grep" in t["name"].lower()])
        nat_grep_count = len([t for t in nat_traces if "grep" in t["name"].lower() or t["name"] == "Grep"])
        fff_read_count = len([t for t in traces if t["name"] == "Read"])
        nat_read_count = len([t for t in nat_traces if t["name"] == "Read"])

        savings = (1 - r["ratio"]) * 100
        print(f"  │   #{r['num']} {r['name']} ({savings:+.0f}%): fff={fff_grep_count}grep+{fff_read_count}read vs nat={nat_grep_count}grep+{nat_read_count}read")

print("  │")
print("  └────────────────────────────────────────────────────────────────────")
print()
ANALYSIS_EOF
fi
