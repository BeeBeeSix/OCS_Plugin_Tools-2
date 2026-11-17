<#
.SYNOPSIS
    Publishes a new version of Tools (OCS_Plugin_Tools).

.DESCRIPTION
    1. Checks the version number and verifies the tag does not exist locally or on GitHub;
    2. Updates the version in Cargo.toml and plugin.toml;
    3. In CHANGELOG.md, moves the entries under "## [Unreleased]" into a new
       section "## [version] - date";
    4. Runs cargo test and cargo build --release;
    5. After confirmation, commits the changes, creates the tag, and pushes to GitHub
       (triggering .github/workflows/release.yml).

    If you answer "no" at the confirmation, all files are restored to their original state.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1 0.1.0

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\release.ps1 0.1.0 -SkipBuild
#>
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,
    # Skip the local tests and build (GitHub Actions runs them anyway).
    [switch]$SkipBuild,
    # Do not ask for confirmation before pushing to GitHub.
    [switch]$Yes
)

Set-StrictMode -Version 2
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root
$Utf8 = New-Object System.Text.UTF8Encoding($false)

function Info($msg) { Write-Host $msg -ForegroundColor Cyan }
function Ok($msg) { Write-Host $msg -ForegroundColor Green }
function Fail($msg) { Write-Host "ERROR: $msg" -ForegroundColor Red; exit 1 }

function Read-Text($path) { [System.IO.File]::ReadAllText((Join-Path $Root $path), $Utf8) }
function Write-Text($path, $text) { [System.IO.File]::WriteAllText((Join-Path $Root $path), $text, $Utf8) }

function Run {
    param([string]$Exe, [string[]]$Arguments)
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "'$Exe $($Arguments -join ' ')' failed (exit code $LASTEXITCODE)." }
}

# ── Checks ───────────────────────────────────────────────────────────────────

$Version = $Version.Trim().TrimStart('v', 'V')
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Fail "Invalid version '$Version': use three numbers, for example 0.1.0"
}
$Tag = "v$Version"

foreach ($tool in 'git', 'cargo') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        if ($tool -eq 'cargo' -and $SkipBuild) { continue }
        Fail "The command '$tool' is not available. Close and reopen PowerShell, or install it."
    }
}
foreach ($f in 'Cargo.toml', 'plugin.toml', 'CHANGELOG.md') {
    if (-not (Test-Path (Join-Path $Root $f))) { Fail "The file $f is missing from the project folder." }
}

$branch = (& git rev-parse --abbrev-ref HEAD).Trim()
if ($branch -ne 'main') { Fail "You are on branch '$branch': switch to 'main' (git checkout main)." }

$prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
& git rev-parse -q --verify "refs/tags/$Tag" 2>&1 | Out-Null
$exists = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = $prev
if ($exists) { Fail "The tag $Tag already exists locally. Use a new version number." }

Info "Checking GitHub..."
$remoteTag = & git ls-remote --tags origin "refs/tags/$Tag"
if ($LASTEXITCODE -ne 0) { Fail "Cannot reach GitHub (git ls-remote). Check your connection." }
if ($remoteTag) { Fail "The tag $Tag already exists on GitHub. Use a new version number." }

& git fetch -q origin main
if ($LASTEXITCODE -eq 0) {
    $behind = (& git rev-list --count HEAD..origin/main).Trim()
    if ([int]$behind -gt 0) {
        Fail "GitHub has $behind change(s) you do not have on this PC. Run first: git pull"
    }
}

$cargo = Read-Text 'Cargo.toml'
$plugin = Read-Text 'plugin.toml'
$changelog = Read-Text 'CHANGELOG.md'
$verRe = '(?m)^(version\s*=\s*")([^"]*)(")'

$m = [regex]::Match($cargo, $verRe)
if (-not $m.Success) { Fail "Cannot find 'version = ...' in Cargo.toml." }
$Current = $m.Groups[2].Value
if ([version]$Version -lt [version]$Current) {
    Fail "The new version ($Version) cannot be lower than the current one ($Current)."
}
if ([version]$Version -eq [version]$Current) {
    Info "Cargo.toml and plugin.toml are already at version ${Version}: publishing that one."
}

# ── CHANGELOG.md ─────────────────────────────────────────────────────────────

$nl = "`n"
if ($changelog.Contains("`r`n")) { $nl = "`r`n" }
$date = Get-Date -Format 'yyyy-MM-dd'

if ($changelog -match ('(?m)^## \[' + [regex]::Escape($Version) + '\]')) {
    Info "CHANGELOG.md already has section ${Version}: using it as it is."
    $newChangelog = $changelog
}
else {
    $head = [regex]::Match($changelog, '(?m)^## \[Unreleased\][ \t]*(?=\r?$)')
    if (-not $head.Success) {
        Fail "CHANGELOG.md has no '## [Unreleased]' heading."
    }
    $after = $changelog.Substring($head.Index + $head.Length)
    $next = [regex]::Match($after, '(?m)^## \[')
    $pending = $after
    if ($next.Success) { $pending = $after.Substring(0, $next.Index) }
    if ($pending.Trim().Length -eq 0) {
        Fail ("The [Unreleased] section of CHANGELOG.md is empty.$nl" +
              "Write this version's changes there (e.g. '### Added' and a '- ...' list) and run again.")
    }
    $insertAt = $head.Index + $head.Length
    $newChangelog = $changelog.Substring(0, $insertAt) + $nl + $nl + "## [$Version] - $date" + $changelog.Substring($insertAt)
}

# ── Updating the files ───────────────────────────────────────────────────────

$originals = @{
    'Cargo.toml'   = $cargo
    'plugin.toml'  = $plugin
    'CHANGELOG.md' = $changelog
}
$lockPath = Join-Path $Root 'Cargo.lock'
$lockOriginal = $null
if (Test-Path $lockPath) { $lockOriginal = Read-Text 'Cargo.lock' }

function Restore-Files {
    foreach ($k in $originals.Keys) { Write-Text $k $originals[$k] }
    if ($null -ne $lockOriginal) { Write-Text 'Cargo.lock' $lockOriginal }
    Info "Files restored as they were: nothing was pushed."
}

$rx = New-Object System.Text.RegularExpressions.Regex($verRe)
Write-Text 'Cargo.toml' ($rx.Replace($cargo, ('${1}' + $Version + '${3}'), 1))
Write-Text 'plugin.toml' ($rx.Replace($plugin, ('${1}' + $Version + '${3}'), 1))
Write-Text 'CHANGELOG.md' $newChangelog
Ok "Version updated: $Current -> $Version (Cargo.toml, plugin.toml, CHANGELOG.md)."

try {
    if (-not $SkipBuild) {
        Info "Running tests..."
        Run 'cargo' @('test', '--release')
        Info "Building release..."
        Run 'cargo' @('build', '--release')
        Ok "Tests and build succeeded."
    }
}
catch {
    Write-Host $_ -ForegroundColor Red
    Restore-Files
    exit 1
}

# ── Summary and confirmation ─────────────────────────────────────────────────

Write-Host ""
Info "Release notes for version $Version (from CHANGELOG.md):"
$sec = [regex]::Match($newChangelog, '(?ms)^## \[' + [regex]::Escape($Version) + '\][^\n]*\n(.*?)(?=^## \[|\z)')
Write-Host $sec.Groups[1].Value.Trim()
Write-Host ""
Info "Modified files:"
& git status --short
Write-Host ""

if (-not $Yes) {
    $ans = Read-Host "Commit, tag '$Tag', and push to GitHub? [y/N]"
    if ($ans -notmatch '^(y|yes)$') {
        Restore-Files
        exit 0
    }
}

try {
    Run 'git' @('add', 'Cargo.toml', 'plugin.toml', 'CHANGELOG.md')
    if (Test-Path $lockPath) { Run 'git' @('add', 'Cargo.lock') }

    $prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    & git diff --cached --quiet
    $hasChanges = ($LASTEXITCODE -ne 0)
    $ErrorActionPreference = $prev

    if ($hasChanges) {
        Run 'git' @('commit', '-m', "Release $Tag")
    } else {
        Info "No file changes to commit; tagging current HEAD as $Tag..."
    }

    Run 'git' @('tag', $Tag)
    Run 'git' @('push', 'origin', 'main')
    Run 'git' @('push', 'origin', $Tag)
    Ok "Pushed tag $Tag to GitHub. Watch the Actions tab for the release build."
}
catch {
    Write-Host $_ -ForegroundColor Red
    Fail "Git commit/tag/push failed. Check 'git status' and 'git tag'."
}
