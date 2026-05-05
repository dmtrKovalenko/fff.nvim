#Requires -Version 5.1
<#
.SYNOPSIS
    FFF MCP Server installer for Windows.
.DESCRIPTION
    Pipe usage:
        irm https://raw.githubusercontent.com/dmtrKovalenko/fff.nvim/main/install-mcp.ps1 | iex
    Direct usage (supports params):
        iwr https://.../install-mcp.ps1 -OutFile install-mcp.ps1; .\install-mcp.ps1 -Version v0.1.2
    Env-var fallbacks (for the piped form):
        $env:FFF_MCP_VERSION, $env:FFF_MCP_INSTALL_DIR
.PARAMETER Version
    Release tag to install (e.g. 'v0.1.2'). Default: latest release containing a Windows MCP asset.
.PARAMETER InstallDir
    Target install directory. Default: $env:LOCALAPPDATA\fff-mcp\bin.
.PARAMETER PathScope
    How to persist PATH: 'User' (set user env var, default), 'Profile' (append to $PROFILE *nix-style), 'None' (do not persist).
    Env-var fallback: $env:FFF_MCP_PATH_SCOPE.
#>
param(
    [string]$Version = $env:FFF_MCP_VERSION,
    [string]$InstallDir = $env:FFF_MCP_INSTALL_DIR,
    [ValidateSet('User', 'Profile', 'None')]
    [string]$PathScope
)
if (-not $PathScope) {
    $PathScope = if ($env:FFF_MCP_PATH_SCOPE) { $env:FFF_MCP_PATH_SCOPE } else { 'User' }
}

$ErrorActionPreference = 'Stop'

# Force TLS 1.2 — PS 5.1 on older Win10 may default to SSL3/TLS1.0 which GitHub rejects.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = 'dmtrKovalenko/fff.nvim'
$BinaryName = 'fff-mcp'
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'fff-mcp\bin' }

function Write-Info    { param($m) Write-Host $m -ForegroundColor Blue }
function Write-Success { param($m) Write-Host $m -ForegroundColor DarkYellow }
function Write-Warn    { param($m) Write-Host $m -ForegroundColor Yellow }

function Get-Target {
    # Read from registry — env vars lie under x86/ARM64 emulation. Same approach Bun uses.
    $arch = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE
    switch ($arch) {
        'AMD64' { return 'x86_64-pc-windows-msvc' }
        'ARM64' { return 'aarch64-pc-windows-msvc' }
        default { throw "Unsupported architecture: $arch" }
    }
}

function Get-LatestReleaseTag {
    param([string]$Target)
    $asset = "$BinaryName-$Target.exe"
    $headers = @{ 'User-Agent' = 'fff-mcp-installer' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }

    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases" -Headers $headers
    $rel = $releases | Where-Object { $_.assets.name -contains $asset } | Select-Object -First 1
    if (-not $rel) {
        throw "No release found containing $asset. The MCP build may not have been released for this platform yet."
    }
    return $rel.tag_name
}

function Invoke-Download {
    param([string]$Url, [string]$OutFile)
    # curl.exe (ships with Win10 1803+) is faster than iwr on PS 5.1. Fall back to iwr.
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source -fsSL -o $OutFile $Url
        if ($LASTEXITCODE -ne 0) { throw "curl.exe exited with $LASTEXITCODE" }
    } else {
        $prev = $ProgressPreference
        try {
            # iwr progress bar tanks throughput on PS 5.1.
            $ProgressPreference = 'SilentlyContinue'
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
        } finally {
            $ProgressPreference = $prev
        }
    }
}

function Install-Binary {
    param([string]$Target, [string]$Tag)

    $filename = "$BinaryName-$Target.exe"
    $url = "https://github.com/$Repo/releases/download/$Tag/$filename"

    Write-Info "Downloading $filename from release $Tag..."

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        $tmpFile = Join-Path $tmp $filename
        try {
            Invoke-Download -Url $url -OutFile $tmpFile
        } catch {
            Write-Host ""
            Write-Host "Error: Failed to download binary for your platform." -ForegroundColor Red
            Write-Host "  URL: $url"
            Write-Host "  Release: $Tag"
            Write-Host "  Platform: $Target"
            Write-Host "Check available releases at: https://github.com/$Repo/releases"
            throw
        }

        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        $dest = Join-Path $InstallDir "$BinaryName.exe"
        Move-Item -Force -Path $tmpFile -Destination $dest
        return $dest
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}

function Test-OnPath {
    param([string]$Dir)
    $paths = $env:PATH -split ';'
    return ($paths -contains $Dir) -or ($paths -contains $Dir.TrimEnd('\'))
}

function Add-ToUserPath {
    param([string]$Dir)
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    $entries = $userPath -split ';' | Where-Object { $_ -ne '' }
    if ($entries -notcontains $Dir) {
        $newPath = (@($entries + $Dir) -join ';')
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Write-Success "Added $Dir to user PATH."
    }
}

function Add-ToProfilePath {
    param([string]$Dir)
    $profilePath = $PROFILE.CurrentUserAllHosts
    $line = "`$env:PATH += ';$Dir'  # added by fff-mcp installer"
    if (Test-Path $profilePath) {
        $existing = Get-Content $profilePath -Raw -ErrorAction SilentlyContinue
        if ($existing -and $existing.Contains($Dir)) { return }
    } else {
        New-Item -ItemType File -Force -Path $profilePath | Out-Null
    }
    Add-Content -Path $profilePath -Value "`n$line"
    Write-Success "Appended PATH update to $profilePath."
}

function Set-Path {
    param([string]$Dir, [string]$Scope)
    switch ($Scope) {
        'User'    { Add-ToUserPath $Dir }
        'Profile' { Add-ToProfilePath $Dir }
        'None'    { Write-Info "Skipping PATH persistence (-PathScope None)." }
    }
    # Make available in current session regardless of scope.
    if (-not (Test-OnPath $Dir)) { $env:PATH = "$env:PATH;$Dir" }
}

function Show-SetupInstructions {
    param([string]$BinaryPath)
    $foundAny = $false

    Write-Host ""
    Write-Success "FFF MCP Server installed successfully!"
    Write-Host ""
    Write-Info "Setup with your AI coding assistant:"
    Write-Host ""

    if (Get-Command claude -ErrorAction SilentlyContinue) {
        $foundAny = $true
        Write-Success "[Claude Code] detected"
        Write-Host ""
        Write-Host "Global (recommended):"
        Write-Host "claude mcp add -s user fff -- $BinaryPath"
        Write-Host ""
        Write-Host "Or project-level .mcp.json (uses PATH):"
        Write-Host @'
{
  "mcpServers": {
    "fff": {
      "type": "stdio",
      "command": "fff-mcp",
      "args": []
    }
  }
}
'@
        Write-Host ""
    }

    if (Get-Command opencode -ErrorAction SilentlyContinue) {
        $foundAny = $true
        Write-Success "[OpenCode] detected"
        Write-Host "Add to your opencode.json:"
        Write-Host @'
{
  "mcp": {
    "fff": {
      "type": "local",
      "command": ["fff-mcp"],
      "enabled": true
    }
  }
}
'@
        Write-Host ""
    }

    if (Get-Command codex -ErrorAction SilentlyContinue) {
        $foundAny = $true
        Write-Success "[Codex] detected"
        Write-Host "codex mcp add fff -- fff-mcp"
        Write-Host ""
    }

    if (-not $foundAny) {
        Write-Host "No AI coding assistants detected."
        Write-Host "Binary path: $BinaryPath"
        Write-Host ""
    }

    Write-Host "Binary: $BinaryPath"
    Write-Host "Docs:   https://github.com/$Repo"
    Write-Host ""
    Write-Info "Tip: Add this to your CLAUDE.md or AGENTS.md to make AI use fff for all searches:"
    Write-Host '"Use the fff MCP tools for all file search operations instead of default tools."'
}

function Main {
    $target = Get-Target

    $existing = Join-Path $InstallDir "$BinaryName.exe"
    $isUpdate = Test-Path $existing

    if ($isUpdate) {
        Write-Info "Updating FFF MCP Server..."
    } else {
        Write-Info "Installing FFF MCP Server..."
    }
    Write-Host ""
    Write-Info "Detected platform: $target"

    if ($Version) {
        $tag = $Version
        Write-Info "Using pinned version: $tag"
    } else {
        $tag = Get-LatestReleaseTag -Target $target
    }
    $binaryPath = Install-Binary -Target $target -Tag $tag

    if ($isUpdate) {
        Write-Host ""
        Write-Success "FFF MCP Server updated to $tag!"
        Write-Host ""
    } else {
        Set-Path -Dir $InstallDir -Scope $PathScope
        Show-SetupInstructions -BinaryPath $binaryPath
    }
}

Main
