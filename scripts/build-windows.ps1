<#
.SYNOPSIS
  Build a release Windows installer of AirNote, baking the DeepSeek meeting-
  summary key into the binary. The Windows counterpart of scripts/build-dmg.sh.

.DESCRIPTION
  DeepSeek is the bundled meeting-AI provider; its key is baked in at compile
  time via option_env!("DEEPSEEK_API_KEY") in meeting_engine.rs (end-users
  cannot change it). option_env! is captured once at compile time, so this
  script:
    1. verifies the toolchain (cargo/rustc/rustup/node/npm + the Rust target),
    2. builds the airnote-backend sidecar (release) and syncs it to the Tauri
       externalBin slot,
    3. ensures the whisper-cli externalBin exists - building it on demand
       (scripts/build-whisper-cli-windows.ps1) if missing, tolerating a
       Vulkan/CMake failure unless -RequireWhisper,
    4. verifies the Silero VAD model (auto-bundled via tauri.conf resources),
    5. pulls DEEPSEEK_API_KEY from the environment or repo-root .env, then
       touches meeting_engine.rs so option_env! re-captures it,
    6. runs `tauri build` and asserts the NSIS installer was produced,
    7. optionally Authenticode-signs the installer.

  Windows has no DMG/codesign/notarization stage, so those Mac-only steps are
  intentionally omitted. Authenticode signing is opt-in via -Sign.

  This script is pure ASCII so it parses identically under Windows PowerShell
  5.1 (ANSI) and PowerShell 7 (UTF-8). Do not add non-ASCII characters.

.PARAMETER Target
  Rust target triple. Default x86_64-pc-windows-msvc.

.PARAMETER SkipWhisper
  Do not build whisper-cli; only warn if the sidecar is missing.

.PARAMETER RebuildWhisper
  Force a whisper-cli rebuild even if the sidecar already exists.

.PARAMETER RequireWhisper
  Fail the build if whisper-cli is missing and cannot be built.

.PARAMETER SkipBackend
  Reuse the existing airnote-backend sidecar; skip the cargo rebuild.

.PARAMETER Clean
  Remove stale release outputs (app exe + NSIS installer) before building.

.PARAMETER Sign
  Authenticode-sign the NSIS installer with signtool. Requires the env var
  AIRNOTE_SIGN_THUMBPRINT (cert thumbprint); AIRNOTE_SIGN_TIMESTAMP_URL is
  optional (defaults to a public RFC-3161 timestamp server).

.PARAMETER RequireInstaller
  Fail if the NSIS installer is not produced (CI gate). Also honoured via the
  AIRNOTE_REQUIRE_INSTALLER=1 environment variable.

.EXAMPLE
  pwsh scripts/build-windows.ps1

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts/build-windows.ps1 -RequireWhisper

.EXAMPLE
  pwsh scripts/build-windows.ps1 -SkipWhisper   # fast app-only iteration
#>
[CmdletBinding()]
param(
  # Only x86_64 is supported today: scripts/build-whisper-cli-windows.ps1 builds
  # an x86_64 whisper-cli only, so an aarch64 bundle would silently ship without
  # meeting transcription. Re-add aarch64 here once that script supports it.
  [ValidateSet('x86_64-pc-windows-msvc')]
  [string]$Target = 'x86_64-pc-windows-msvc',
  [switch]$SkipWhisper,
  [switch]$RebuildWhisper,
  [switch]$RequireWhisper,
  [switch]$SkipBackend,
  [switch]$Clean,
  [switch]$Sign,
  [switch]$RequireInstaller
)

$ErrorActionPreference = 'Stop'

# ---- Paths ------------------------------------------------------------------
$RepoRoot    = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$DesktopDir  = Join-Path $RepoRoot 'desktop'
$TauriDir    = Join-Path $DesktopDir 'src-tauri'
$BinariesDir = Join-Path $TauriDir 'binaries'
$MeetingRs   = Join-Path $TauriDir 'src\meeting_engine.rs'
$BackendMain = Join-Path $RepoRoot 'crates\backend\src\main.rs'
$SileroSrc   = Join-Path $TauriDir 'resources\models\ggml-silero-v5.1.2.bin'
$ReleaseDir  = Join-Path $RepoRoot ("target\{0}\release" -f $Target)
$BundleDir   = Join-Path $ReleaseDir 'bundle'
$NsisDir     = Join-Path $BundleDir 'nsis'
$SidecarSrc  = Join-Path $ReleaseDir 'airnote-backend.exe'
$SidecarDest = Join-Path $BinariesDir ("airnote-backend-{0}.exe" -f $Target)
$WhisperSrc  = Join-Path $ReleaseDir 'whisper-cli.exe'
$WhisperDest = Join-Path $BinariesDir ("whisper-cli-{0}.exe" -f $Target)
$AppExe      = Join-Path $ReleaseDir 'AirNote.exe'
$WhisperBuildScript = Join-Path $PSScriptRoot 'build-whisper-cli-windows.ps1'
$MinSileroBytes = 100000   # mirrors MIN_SILERO_VAD_BYTES in meeting_engine.rs

# ---- Output helpers ---------------------------------------------------------
function Step($m) { Write-Host "`n> $m" -ForegroundColor White }
function OK($m)   { Write-Host "  + $m"  -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m"  -ForegroundColor Yellow }
function Fail($m) { Write-Host "`n  x $m`n" -ForegroundColor Red; exit 1 }

# `touch` equivalent - bumps mtime to now so Cargo's incremental fingerprint
# recompiles the file (needed to re-bake option_env!). Fails loudly if absent.
function Touch($path) {
  if (-not (Test-Path $path)) { Fail "cannot touch (missing file): $path" }
  (Get-Item $path).LastWriteTime = Get-Date
}

function Require-Command($cmd, $hint) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
    Fail "'$cmd' not found on PATH. $hint"
  }
}

# Secret-safe single-key .env reader. Handles CRLF, leading whitespace, an
# optional 'export ' prefix, spaces around '=', surrounding single/double
# quotes, and a trailing '# comment' on unquoted values. Returns $null if the
# key/file is absent. NEVER prints the value.
function Get-EnvValue($Key, $Path) {
  if (-not (Test-Path $Path)) { return $null }
  $prefix = '^' + [regex]::Escape($Key) + '\s*=\s*'
  foreach ($raw in @(Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue)) {
    $line = ($raw -replace '\r$', '').TrimStart()
    if ($line.Length -eq 0 -or $line.StartsWith('#')) { continue }
    $line = $line -replace '^export\s+', ''
    if ($line -match $prefix) {
      $val = $line -replace $prefix, ''
      if ($val -match '^"(.*?)"') { return $Matches[1] }
      if ($val -match "^'(.*?)'") { return $Matches[1] }
      return ($val -replace '\s+#.*$', '').Trim()
    }
  }
  return $null
}

# ---- Preflight: toolchain ---------------------------------------------------
Step "Preflight: verify toolchain"
Require-Command 'cargo'  'Install Rust from https://rustup.rs'
Require-Command 'rustc'  'Install Rust from https://rustup.rs'
Require-Command 'rustup' 'Install Rust from https://rustup.rs'
Require-Command 'node'   'Install Node.js from https://nodejs.org'
Require-Command 'npm'    'Install Node.js from https://nodejs.org'
$installedTargets = @(rustup target list --installed)   # force array (single target -> scalar otherwise)
if ($LASTEXITCODE -ne 0) { Fail "rustup target list failed" }
if ($installedTargets -notcontains $Target) {
  Fail "Rust target $Target not installed. Run: rustup target add $Target"
}
OK "cargo / rustc / node / npm present; Rust target $Target installed"

Set-Location $RepoRoot

# Read the workspace version (single source of truth) for the closing summary.
$Version = ''
try {
  $inSection = $false
  foreach ($l in (Get-Content (Join-Path $RepoRoot 'Cargo.toml'))) {
    if ($l -match '^\[workspace\.package\]') { $inSection = $true; continue }
    if ($l -match '^\[') { $inSection = $false }
    if ($inSection -and $l -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
} catch {}

# ---- Optional clean ---------------------------------------------------------
# Removes stale outputs so the final "installer produced?" assertion is honest
# (a leftover installer from a prior run must not be mistaken for a fresh one).
if ($Clean) {
  Step "Clean stale release outputs"
  foreach ($p in @($AppExe, $NsisDir)) {
    if (Test-Path $p) { Remove-Item -Recurse -Force $p; OK "removed $p" }
  }
}

# ---- Bundle build-time keys (option_env!) -----------------------------------
# said-core bakes DEEPGRAM_API_KEY (Cloud dictation) and said-desktop bakes
# DEEPSEEK_API_KEY (meeting summaries) via option_env!, so the shipped app works
# without users entering any API key (the key-entry UI was removed). Note polish
# runs server-side (Cerebras via the server runtime), so NO gateway/LLM key is
# bundled - this matches build-dmg.sh (which dropped GATEWAY_API_KEY in 5814e82).
# Keys must be set BEFORE the backend build (said-core compiles there) and stay
# set through the tauri build; the crates' build.rs rerun-if-env-changed
# directives re-bake on change. Loaded from repo-root .env, then unset after the
# build so they do not leak into the caller's session.
Step "Bundle build-time keys (option_env!)"
$EnvFile = Join-Path $RepoRoot '.env'
$ScriptSetKeys = @()
$BundledKeys = @(
  @{ name = 'DEEPSEEK_API_KEY'; purpose = 'meeting summaries' },
  @{ name = 'DEEPGRAM_API_KEY'; purpose = 'Cloud dictation' }
)
foreach ($k in $BundledKeys) {
  $n = $k.name
  if (-not [Environment]::GetEnvironmentVariable($n)) {
    $v = Get-EnvValue $n $EnvFile
    if ($v) { [Environment]::SetEnvironmentVariable($n, $v); $ScriptSetKeys += $n }
  }
  $cur = [Environment]::GetEnvironmentVariable($n)
  if ($cur) { OK "$n will be bundled (length $($cur.Length); value not shown)" }
  else { Warn "$n not set (env or .env) - $($k.purpose) will FAIL in the build until it is added to .env." }
}
# Belt-and-suspenders re-bake of the said-desktop (DeepSeek) option_env! site;
# said-core's keys re-bake via crates/core/build.rs rerun-if-env-changed.
Touch $MeetingRs

# ---- Build the Rust sidecar (release) ---------------------------------------
if ($SkipBackend) {
  Step "Skip airnote-backend build (-SkipBackend)"
  if (-not (Test-Path $SidecarDest)) {
    Fail "-SkipBackend set but no existing sidecar at $SidecarDest. Run once without -SkipBackend first."
  }
  OK "reusing existing sidecar: binaries\airnote-backend-$Target.exe"
} else {
  Step "Build airnote-backend (release, $Target)"
  Touch $BackendMain   # bust the Cargo fingerprint for the entry point
  cargo build -p said-backend --bin airnote-backend --release --target $Target
  if ($LASTEXITCODE -ne 0) { Fail "cargo build failed for airnote-backend" }
  if (-not (Test-Path $SidecarSrc)) { Fail "airnote-backend.exe not found at $SidecarSrc" }
  New-Item -ItemType Directory -Force -Path $BinariesDir | Out-Null
  try {
    Copy-Item $SidecarSrc $SidecarDest -Force
  } catch {
    Fail "could not write $SidecarDest - is AirNote.exe still running? Close it and re-run. ($_)"
  }
  OK "synced sidecar -> binaries\airnote-backend-$Target.exe"
}

# ---- whisper-cli sidecar: verify / build on demand / sync -------------------
Step "Resolve whisper-cli externalBin"
$haveWhisper = Test-Path $WhisperDest
if ($haveWhisper -and -not $RebuildWhisper) {
  OK "whisper-cli present: binaries\whisper-cli-$Target.exe"
} elseif ($SkipWhisper) {
  if ($haveWhisper) {
    OK "whisper-cli present (skip rebuild)"
  } else {
    Warn "whisper-cli missing and -SkipWhisper set - meetings will NOT transcribe in this build."
  }
} else {
  if ($RebuildWhisper) {
    Step "Rebuild whisper-cli (-RebuildWhisper)"
  } else {
    Step "whisper-cli missing - building it (scripts\build-whisper-cli-windows.ps1)"
  }

  if (-not (Test-Path $WhisperBuildScript)) { Fail "whisper build script not found: $WhisperBuildScript" }
  Require-Command 'git'   'whisper-cli build needs Git (https://git-scm.com)'
  Require-Command 'cmake' 'whisper-cli build needs CMake (winget install Kitware.CMake)'

  # Run the whisper build in a CHILD PowerShell so its internal `exit 1` cannot
  # terminate THIS script - that lets us fall back to a warning (or fail only
  # when -RequireWhisper). The build auto-detects Vulkan and degrades to CPU.
  $psExe = (Get-Process -Id $PID -ErrorAction SilentlyContinue).Path
  if (-not $psExe) {
    $psExe = if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh' } else { 'powershell' }
  }
  $whisperOk = $false
  try {
    & $psExe -NoProfile -ExecutionPolicy Bypass -File $WhisperBuildScript -Target $Target
    if ($LASTEXITCODE -eq 0) { $whisperOk = $true }
  } catch { $whisperOk = $false }

  if ($whisperOk -and (Test-Path $WhisperSrc)) {
    New-Item -ItemType Directory -Force -Path $BinariesDir | Out-Null
    Copy-Item $WhisperSrc $WhisperDest -Force
    OK "built + synced whisper-cli -> binaries\whisper-cli-$Target.exe"
  } else {
    $msg = "whisper-cli build did not produce a binary (Vulkan SDK / CMake / network?). See output above."
    if ($RequireWhisper) {
      Fail "$msg  (-RequireWhisper)"
    } elseif ($haveWhisper) {
      Warn "$msg  Keeping the existing whisper-cli sidecar."
    } else {
      Warn "$msg  Meetings will NOT transcribe. Install the Vulkan SDK (winget install KhronosGroup.VulkanSDK) or run scripts\build-whisper-cli-windows.ps1 manually, then re-run."
    }
  }
}

# ---- Verify Silero VAD model ------------------------------------------------
# Declared in tauri.conf.json resources, so Tauri auto-bundles it into the NSIS
# installer - no manual staging needed. We only verify the source exists, and
# self-heal from the whisper build's download dir if it is missing.
Step "Verify Silero VAD model (auto-bundled via tauri.conf resources)"
if ((Test-Path $SileroSrc) -and ((Get-Item $SileroSrc).Length -ge $MinSileroBytes)) {
  OK "Silero model present: resources\models\ggml-silero-v5.1.2.bin"
} else {
  $fallback = Join-Path $RepoRoot 'target\whisper-models\ggml-silero-v5.1.2.bin'
  if ((Test-Path $fallback) -and ((Get-Item $fallback).Length -ge $MinSileroBytes)) {
    New-Item -ItemType Directory -Force -Path (Split-Path $SileroSrc) | Out-Null
    Copy-Item $fallback $SileroSrc -Force
    OK "restored Silero model from target\whisper-models into resources\models"
  } else {
    Warn "Silero VAD model missing/too small at resources\models - meetings will run WITHOUT VAD. Build whisper-cli (scripts\build-whisper-cli-windows.ps1) to fetch it."
  }
}

# ---- Updater artifact signing ----------------------------------------------
# tauri.conf has an updater pubkey + createUpdaterArtifacts=true, so tauri tries
# to sign the updater bundle with TAURI_SIGNING_PRIVATE_KEY. If that key is
# available (env or .env) pass it through for a full production build with a
# working auto-update signature. If not, disable updater artifacts so the build
# still SUCCEEDS and produces a usable installer (just no auto-update .sig) -
# otherwise tauri exits 1 AFTER bundling. Mirrors release.yml (signs) and
# build-dmg.sh (disables for local). The installer itself is identical either way.
Step "Updater artifact signing"
$ScriptSetSignKey = $false
$ScriptSetSignPwd = $false
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
  $sk = Get-EnvValue 'TAURI_SIGNING_PRIVATE_KEY' (Join-Path $RepoRoot '.env')
  if ($sk) { $env:TAURI_SIGNING_PRIVATE_KEY = $sk; $ScriptSetSignKey = $true }
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
  $skp = Get-EnvValue 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD' (Join-Path $RepoRoot '.env')
  if ($skp) { $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $skp; $ScriptSetSignPwd = $true }
}
$NoUpdaterCfg = $null
if ($env:TAURI_SIGNING_PRIVATE_KEY) {
  OK "updater artifacts will be SIGNED (TAURI_SIGNING_PRIVATE_KEY present)"
} else {
  Warn "TAURI_SIGNING_PRIVATE_KEY not set - disabling updater artifacts (installer still works; no auto-update signature)."
  # Pass the override as a temp file, not inline JSON, to dodge npm/PowerShell quoting.
  $NoUpdaterCfg = Join-Path ([System.IO.Path]::GetTempPath()) ("airnote-noupdater-{0}.json" -f $PID)
  Set-Content -LiteralPath $NoUpdaterCfg -Value '{"bundle":{"createUpdaterArtifacts":false}}' -Encoding ASCII
}

# ---- Tauri build ------------------------------------------------------------
Step "Run tauri build (--target $Target)"
$cwd = (Get-Location).Path
try {
  Set-Location $DesktopDir
  if (-not (Test-Path 'node_modules')) {
    Step "Install npm dependencies (node_modules missing)"
    npm ci
    if ($LASTEXITCODE -ne 0) { Fail "npm ci failed" }
  }
  if ($NoUpdaterCfg) {
    npm run tauri:build -- --target $Target --config $NoUpdaterCfg
  } else {
    npm run tauri:build -- --target $Target
  }
  if ($LASTEXITCODE -ne 0) { Fail "tauri build failed" }
  OK "tauri build finished"
} finally {
  Set-Location $cwd
  # Keys were needed only to bake into the said-core/said-desktop compiles during
  # this build. Don't leak them into the caller's session or later child
  # processes; only unset what we loaded from .env ourselves (leave any ambient
  # ones the user set).
  foreach ($n in $ScriptSetKeys) { [Environment]::SetEnvironmentVariable($n, $null) }
  if ($ScriptSetSignKey)  { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue }
  if ($ScriptSetSignPwd)  { Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue }
  if ($NoUpdaterCfg -and (Test-Path $NoUpdaterCfg)) { Remove-Item -LiteralPath $NoUpdaterCfg -Force -ErrorAction SilentlyContinue }
}

# ---- Verify (and optionally sign) the installer -----------------------------
Step "Verify NSIS installer"
$Nsis = Get-ChildItem -Path $NsisDir -Filter '*.exe' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $Nsis) {
  if ($RequireInstaller -or $env:AIRNOTE_REQUIRE_INSTALLER -eq '1') {
    Fail "NSIS installer not found in $NsisDir - tauri build produced no installer."
  }
  Warn "NSIS installer not found in $NsisDir (check the tauri build output above)."
} else {
  $sizeMb = [math]::Round($Nsis.Length / 1MB, 1)
  OK "installer: $($Nsis.FullName) ($sizeMb MB)"
  if ($Sign) {
    Step "Authenticode-sign the installer"
    Require-Command 'signtool' 'Install the Windows SDK (signtool.exe), then re-run with -Sign.'
    $thumb = $env:AIRNOTE_SIGN_THUMBPRINT
    if (-not $thumb) { Fail "-Sign requires the AIRNOTE_SIGN_THUMBPRINT env var (signing cert thumbprint)." }
    $ts = if ($env:AIRNOTE_SIGN_TIMESTAMP_URL) { $env:AIRNOTE_SIGN_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    signtool sign /fd SHA256 /sha1 $thumb /tr $ts /td SHA256 $Nsis.FullName
    if ($LASTEXITCODE -ne 0) { Fail "signtool signing failed" }
    OK "installer signed (timestamp $ts)"
  }
}

# ---- Summary ----------------------------------------------------------------
$VersionLabel = if ($Version) { " (v$Version)" } else { '' }
Step "Done$VersionLabel"
if (Test-Path $AppExe) { Write-Host "  app exe:   $AppExe" }
if ($Nsis)             { Write-Host "  installer: $($Nsis.FullName)" }
Write-Host ""
Write-Host "  Install: run the NSIS installer above (or launch the app exe directly for a quick test)."
Write-Host ""
