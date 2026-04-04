#!/usr/bin/env bash
set -eo pipefail

# FFF MCP Server installer
# Usage: curl -fsSL https://raw.githubusercontent.com/dmtrKovalenko/fff.nvim/main/install-mcp.sh | bash
#
# Warp terminal integration note:
#   Warp serializes integer MCP parameters as IEEE 754 floats (e.g. maxResults: 30.0).
#   The fff-mcp server handles this transparently via a lenient usize deserializer
#   so no special workaround is needed on the client side.

REPO="dmtrKovalenko/fff.nvim"
BINARY_NAME="fff-mcp"
INSTALL_DIR="${FFF_MCP_INSTALL_DIR:-$HOME/.local/bin}"

info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
success() { printf '\033[1;38;5;208m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*"; }
error() { printf '\033[1;31mError: %s\033[0m\n' "$*" >&2; exit 1; }

# Print JSON with syntax highlighting via jq if available, plain otherwise
print_json() {
    if command -v jq &>/dev/null; then
        echo "$1" | jq .
    else
        echo "$1"
    fi
}

detect_platform() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)
            # Prefer musl (static) for maximum compatibility
            case "$arch" in
                x86_64)  target="x86_64-unknown-linux-musl" ;;
                aarch64|arm64) target="aarch64-unknown-linux-musl" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64)  target="x86_64-apple-darwin" ;;
                aarch64|arm64) target="aarch64-apple-darwin" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        MINGW*|MSYS*|CYGWIN*)
            case "$arch" in
                x86_64)  target="x86_64-pc-windows-msvc" ;;
                aarch64|arm64) target="aarch64-pc-windows-msvc" ;;
                *) error "Unsupported architecture: $arch" ;;
            esac
            ;;
        *) error "Unsupported OS: $os" ;;
    esac

    echo "$target"
}

get_latest_release_tag() {
    local target="$1"
    local releases_json
    releases_json=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases") \
        || error "Failed to fetch releases from https://github.com/${REPO}/releases"

    # Find the first release that contains an fff-mcp binary for our platform
    local tag
    tag=$(echo "$releases_json" \
        | grep -oE '"(tag_name|name)": *"[^"]*"' \
        | awk -v target="fff-mcp-${target}" '
            /"tag_name":/ { gsub(/.*": *"|"/, ""); current_tag = $0; next }
            /"name":/ && index($0, target) { print current_tag; exit }
        ')

    if [ -z "$tag" ]; then
        error "No release found containing fff-mcp binaries for ${target}. The MCP build may not have been released yet."
    fi
    echo "$tag"
}

download_binary() {
    local target="$1"
    local tag="$2"
    local ext=""

    case "$target" in
        *windows*) ext=".exe" ;;
    esac

    local filename="${BINARY_NAME}-${target}${ext}"
    local url="https://github.com/${REPO}/releases/download/${tag}/${filename}"
    local checksum_url="${url}.sha256"

    info "Downloading ${filename} from release ${tag}..."

    local tmp_dir
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf "$tmp_dir"' EXIT

    if ! curl -fsSL -o "${tmp_dir}/${filename}" "$url" 2>/dev/null; then
        echo "" >&2
        printf '\033[1;31mError: Failed to download binary for your platform.\033[0m\n' >&2
        echo "" >&2
        echo "  URL: ${url}" >&2
        echo "  Release: ${tag}" >&2
        echo "  Platform: ${target}" >&2
        echo "" >&2
        echo "This likely means the MCP binary hasn't been built for this release yet." >&2
        echo "Check available releases at: https://github.com/${REPO}/releases" >&2
        exit 1
    fi

    # Verify checksum if sha256sum is available
    if command -v sha256sum &>/dev/null; then
        if curl -fsSL -o "${tmp_dir}/${filename}.sha256" "$checksum_url" 2>/dev/null; then
            info "Verifying checksum..."
            (cd "$tmp_dir" && sha256sum -c "${filename}.sha256") \
                || error "Checksum verification failed!"
        else
            warn "Checksum file not available, skipping verification."
        fi
    fi

    # Install
    mkdir -p "$INSTALL_DIR"
    mv "${tmp_dir}/${filename}" "${INSTALL_DIR}/${BINARY_NAME}${ext}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}${ext}"

    if [ "$IS_UPDATE" != true ]; then
        success "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}${ext}"
    fi
}

check_path() {
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) return 0 ;;
    esac

    warn "${INSTALL_DIR} is not in your PATH."
    echo ""
    echo "Add it to your shell profile:"
    echo ""

    local shell_name
    shell_name="$(basename "${SHELL:-bash}")"
    case "$shell_name" in
        zsh)
            echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc"
            echo "  source ~/.zshrc"
            ;;
        fish)
            echo "  fish_add_path ${INSTALL_DIR}"
            ;;
        *)
            echo "  echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc"
            echo "  source ~/.bashrc"
            ;;
    esac
    echo ""
}

print_setup_instructions() {
    local binary_path="${INSTALL_DIR}/${BINARY_NAME}"
    local found_any=false

    echo ""
    success "FFF MCP Server installed successfully!"
    echo ""
    info "Setup with your AI coding assistant:"
    echo ""

    # Claude Code
    if command -v claude &>/dev/null; then
        found_any=true
        success "[Claude Code] detected"
        echo ""
        echo "Global (recommended):"
        echo "claude mcp add -s user fff -- ${binary_path}"
        echo ""
        echo "Or project-level .mcp.json (uses PATH):"
        echo ""
        print_json '{
  "mcpServers": {
    "fff": {
      "type": "stdio",
      "command": "fff-mcp",
      "args": []
    }
  }
}'
        echo ""
    fi

    # OpenCode
    if command -v opencode &>/dev/null; then
        found_any=true
        success "[OpenCode] detected"
        echo ""
        echo "Add to ~/.config/opencode/opencode.json:"
        echo ""
        print_json '{
  "mcp": {
    "fff": {
      "type": "local",
      "command": ["fff-mcp"],
      "enabled": true
    }
  }
}'
        echo ""
    fi

    # Codex
    if command -v codex &>/dev/null; then
        found_any=true
        success "[Codex] detected"
        echo ""
        echo "codex mcp add fff -- fff-mcp"
        echo ""
    fi

    # Warp terminal
    # Detection: running inside Warp ($TERM_PROGRAM) or Warp data dir exists.
    # Warp uses the mcpServers JSON format; paste via Settings → AI → MCP Servers → + Add.
    if [ "${TERM_PROGRAM}" = "WarpTerminal" ] \
        || [ -d "$HOME/Library/Application Support/dev.warp.Warp-Stable" ] \
        || [ -d "${XDG_STATE_HOME:-$HOME/.local/state}/warp-terminal" ]; then
        found_any=true
        success "[Warp] detected"
        echo ""
        echo "Go to Settings → AI → MCP Servers (or Warp Drive → MCP Servers) and click + Add."
        echo "Paste this configuration:"
        echo ""
        print_json '{
  "mcpServers": {
    "fff": {
      "command": "fff-mcp",
      "args": []
    }
  }
}'
        echo ""
        info "Tip: Create a Warp Drive Rule to always route file searches through fff:"
        echo "  Warp Drive → Rules → New Rule:"
        echo "  \"Use the fff MCP tools for all file search operations instead of default tools.\""
        echo ""
    fi

    if [ "$found_any" = false ]; then
        echo "No AI coding assistants detected."
        echo ""
        echo "Binary path: ${binary_path}"
        echo ""
    fi

    echo "Binary: ${binary_path}"
    echo "Docs:   https://github.com/${REPO}"
    echo ""
    info "Tip: Add this to your CLAUDE.md or AGENTS.md to make AI use fff for all searches:"
    echo "\""
    echo "Use the fff MCP tools for all file search operations instead of default tools."
    echo "\""


}

main() {
    local target
    target="$(detect_platform)"

    local existing_binary="${INSTALL_DIR}/${BINARY_NAME}"
    IS_UPDATE=false

    if [ -x "$existing_binary" ]; then
        IS_UPDATE=true
        info "Updating FFF MCP Server..."
    else
        info "Installing FFF MCP Server..."
    fi
    echo ""

    info "Detected platform: ${target}"

    local tag
    tag="$(get_latest_release_tag "$target")"

    download_binary "$target" "$tag"

    if [ "$IS_UPDATE" = true ]; then
        echo ""
        success "FFF MCP Server updated to ${tag}!"
        echo ""
    else
        check_path
        print_setup_instructions
    fi
}

main
