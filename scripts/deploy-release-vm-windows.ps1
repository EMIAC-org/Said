<#
.SYNOPSIS
  Publish a signed Windows release to the AirNote updater host - the Windows
  counterpart of scripts/deploy-release-vm.sh (which is intentionally Mac-only).

.DESCRIPTION
  Signs the locally built NSIS installer with the Tauri updater key, writes the
  PER-PLATFORM Windows updater manifest, uploads the installer + signature +
  manifest to the VM, and prunes old releases. Run scripts/build-windows.ps1
  first to produce target/<triple>/release/bundle/nsis/AirNote_<ver>_x64-setup.exe.

  By default it ONLY touches the modern per-platform channel
  (updates/windows/latest.json) - safe to run every release.

  -MigrateLegacy ALSO merges a windows-x86_64 entry into the legacy COMBINED
  manifest (updates/latest.json) via read-modify-write, PRESERVING the existing
  darwin entry. This is what lets stuck <=2.2.9 Windows users (which only poll
  the combined manifest) finally auto-update. It is a one-time migration aid -
  release.yml deliberately never rewrites the combined manifest on a single-
  platform release, so this is opt-in and preserves the other platform's entry.
  After a 2.2.9 client takes this update it lands on 2.3.0+ which polls the
  per-platform channel, so it will not loop on the combined manifest.

  Tauri v2 NSIS updater artifact = the signed -setup.exe (+ -setup.exe.sig);
  updater platform key = windows-x86_64.

  Always start with -DryRun to inspect the manifests and the exact upload plan
  before touching the live server.

.PARAMETER Target
  Rust target triple. Default x86_64-pc-windows-msvc.

.PARAMETER MigrateLegacy
  Also merge windows-x86_64 into the legacy combined updates/latest.json
  (preserving darwin). Use once to unstick the <=2.2.9 Windows base.

.PARAMETER DryRun
  Build and print the manifests + the upload plan, but do NOT sign-upload or
  mutate the server. (Signing still runs so you can inspect the .sig.)

.EXAMPLE
  $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $HOME\.tauri\said-updater.key -Raw
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '...'
  pwsh scripts/deploy-release-vm-windows.ps1 -DryRun
  pwsh scripts/deploy-release-vm-windows.ps1               # per-platform only
  pwsh scripts/deploy-release-vm-windows.ps1 -MigrateLegacy  # + unstick 2.2.9
#>
[CmdletBinding()]
param(
  [ValidateSet('x86_64-pc-windows-msvc')]
  [string]$Target = 'x86_64-pc-windows-msvc',
  [switch]$MigrateLegacy,
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

# ---- Config (env-overridable, same defaults as deploy-release-vm.sh) --------
$ProductName    = if ($env:PRODUCT_NAME) { $env:PRODUCT_NAME } else { 'AirNote' }
$PublicBaseUrl  = if ($env:PUBLIC_BASE_URL) { $env:PUBLIC_BASE_URL } else { 'https://airnote.emiactech.com' }
$Remote         = if ($env:REMOTE) { $env:REMOTE } else { 'root@103.180.163.41' }
$RemoteRoot     = if ($env:REMOTE_RELEASE_ROOT) { $env:REMOTE_RELEASE_ROOT } else { '/opt/airnote-control-plane/releases' }
$KeepReleases   = if ($env:KEEP_RELEASES) { [int]$env:KEEP_RELEASES } else { 3 }
$PlatformKey    = 'windows-x86_64'

$RepoRoot  = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$ReleaseDir = Join-Path $RepoRoot ("target\{0}\release" -f $Target)
$NsisDir   = Join-Path $ReleaseDir 'bundle\nsis'

function Step($m) { Write-Host "`n> $m" -ForegroundColor White }
function OK($m)   { Write-Host "  + $m"  -ForegroundColor Green }
function Warn($m) { Write-Host "  ! $m"  -ForegroundColor Yellow }
function Fail($m) { Write-Host "`n  x $m`n" -ForegroundColor Red; exit 1 }
function Require-Command($cmd, $hint) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) { Fail "'$cmd' not found on PATH. $hint" }
}

# ---- Resolve version + artifact ---------------------------------------------
Step "Resolve version + installer"
$Version = ''
$inSection = $false
foreach ($l in (Get-Content (Join-Path $RepoRoot 'Cargo.toml'))) {
  if ($l -match '^\[workspace\.package\]') { $inSection = $true; continue }
  if ($l -match '^\[') { $inSection = $false }
  if ($inSection -and $l -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { Fail "could not parse [workspace.package].version from Cargo.toml" }

# Release channel (matches release.yml): stable -> latest, prerelease -> beta.
# The per-platform manifest is published as updates/windows/<channel>.json.
$Channel = 'latest'
if ($Version -match '-(beta|rc|alpha)') { $Channel = 'beta' }
if ($MigrateLegacy -and $Channel -ne 'latest') {
  Fail "-MigrateLegacy unsticks the stable <=2.2.9 base via updates/latest.json; refusing to point them at a $Channel build ($Version). Run -MigrateLegacy only on a stable release."
}

$SetupName = "{0}_{1}_x64-setup.exe" -f $ProductName, $Version
$Installer = Join-Path $NsisDir $SetupName
$SigFile   = "$Installer.sig"
if (-not (Test-Path $Installer)) {
  Fail "installer not found: $Installer`n    Run scripts/build-windows.ps1 first."
}
OK "version $Version; installer $SetupName"

# ---- Sign the updater artifact ----------------------------------------------
# The deploy step owns signing (mirrors deploy-release-vm.sh), so the build does
# not need the key. tauri signer writes <installer>.sig next to the installer.
Step "Sign updater artifact"
Require-Command 'npx' 'Install Node.js (https://nodejs.org)'
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
  Fail "set TAURI_SIGNING_PRIVATE_KEY (the minisign updater private key) - e.g. `$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content `$HOME\.tauri\said-updater.key -Raw"
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
  Fail "set TAURI_SIGNING_PRIVATE_KEY_PASSWORD (the key's password)"
}
$cwd = (Get-Location).Path
try {
  Set-Location (Join-Path $RepoRoot 'desktop')
  npx tauri signer sign $Installer --private-key $env:TAURI_SIGNING_PRIVATE_KEY --password $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  if ($LASTEXITCODE -ne 0) { Fail "tauri signer sign failed" }
} finally {
  Set-Location $cwd
}
if (-not (Test-Path $SigFile)) { Fail "signature not produced: $SigFile" }
$Signature = (Get-Content -LiteralPath $SigFile -Raw).Trim()
if (-not $Signature) { Fail "signature file is empty: $SigFile" }
OK "signed -> $SetupName.sig"

# ---- Build the per-platform Windows manifest --------------------------------
# Exact format mirrors release.yml's write_manifest. The url must be reachable
# by clients (same VM via the public host).
$PubDate    = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$ArtifactUrl = "$PublicBaseUrl/releases/$Version/$SetupName"
$Notes      = "$ProductName $Version"

# Build via PSCustomObject so ConvertTo-Json escapes the (long, base64) values
# correctly. Depth high enough for the nested platforms map.
function New-Manifest($platforms) {
  [pscustomobject][ordered]@{
    version   = $Version
    notes     = $Notes
    pub_date  = $PubDate
    platforms = $platforms
  } | ConvertTo-Json -Depth 8
}
$WindowsEntry = [ordered]@{ signature = $Signature; url = $ArtifactUrl }
$PerPlatformManifest = New-Manifest ([ordered]@{ "$PlatformKey" = $WindowsEntry })

$StageDir = Join-Path ([System.IO.Path]::GetTempPath()) ("airnote-windeploy-{0}" -f $Version)
if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir }
New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
$PerPlatformName = "windows-$Channel.json"            # staged name (mirrors release.yml)
$PerPlatformPath = Join-Path $StageDir $PerPlatformName
# UTF-8 WITHOUT BOM (Set-Content -Encoding UTF8 on PS5.1 adds a BOM that can
# break the updater's JSON parser).
[System.IO.File]::WriteAllText($PerPlatformPath, $PerPlatformManifest)
Step "Per-platform manifest (updates/windows/$Channel.json)"
Write-Host $PerPlatformManifest

# ---- Optionally merge the legacy COMBINED manifest (preserve darwin) --------
$CombinedPath = $null
if ($MigrateLegacy) {
  Step "Merge legacy combined manifest (updates/latest.json) - preserving darwin"
  $combinedUrl = "$PublicBaseUrl/updates/latest.json"
  $existing = $null
  try {
    $existing = Invoke-RestMethod -Uri $combinedUrl -TimeoutSec 20
  } catch {
    Warn "could not fetch current combined manifest ($combinedUrl): $($_.Exception.Message)"
  }

  # Read existing platform entries. Must have a non-empty platforms object
  # (note: an empty {} is still "truthy", so check the property COUNT).
  $existingProps = @()
  if ($existing -and $existing.platforms) {
    $existingProps = @($existing.platforms.PSObject.Properties)
  }
  if ($existingProps.Count -eq 0) {
    Warn "current combined manifest at $combinedUrl has no platform entries (unreachable, empty, or malformed)."
    Warn "ABORTING: publishing a windows-only combined would DROP the macOS entry and break Mac legacy clients."
    Fail "combined merge aborted - could not read the existing darwin entry to preserve it."
  }
  # Preserve every existing platform entry VERBATIM (all fields, not just two),
  # so a future manifest field is never silently dropped.
  $platforms = [ordered]@{}
  foreach ($p in $existingProps) { $platforms[$p.Name] = $p.Value }
  Write-Host ("  existing combined platforms: {0}" -f (($existingProps.Name) -join ', '))

  # Hard guard: the whole point of -MigrateLegacy is to ADD windows WITHOUT
  # losing macOS. If darwin is absent from the source, refuse rather than ship a
  # combined manifest that strands Mac legacy clients.
  if ($platforms.Keys -notcontains 'darwin-aarch64') {
    Fail "existing combined manifest has no darwin-aarch64 entry - refusing to write a combined manifest that would drop macOS. Verify $combinedUrl."
  }

  # Add / replace ONLY the windows entry.
  $platforms[$PlatformKey] = $WindowsEntry

  # Version field: keep it >= the highest present so it still triggers 2.2.9.
  # Safe because darwin is guaranteed present above: a Mac straggler downloads
  # its own (possibly older) darwin url, then 2.3.0+ moves to the per-platform
  # channel and stops polling this combined manifest.
  $combinedVersion = $Version
  if ($existing -and $existing.version) {
    try {
      if ([version]$existing.version -gt [version]$Version) { $combinedVersion = $existing.version }
    } catch {}
  }
  $CombinedManifest = [pscustomobject][ordered]@{
    version   = $combinedVersion
    notes     = $Notes
    pub_date  = $PubDate
    platforms = $platforms
  } | ConvertTo-Json -Depth 8
  $CombinedPath = Join-Path $StageDir 'latest.json'
  [System.IO.File]::WriteAllText($CombinedPath, $CombinedManifest)
  Write-Host $CombinedManifest
  # Post-write assertion: never report success unless BOTH platforms survived.
  $merged = (Get-Content -LiteralPath $CombinedPath -Raw | ConvertFrom-Json).platforms
  if (-not $merged.'darwin-aarch64' -or -not $merged.'windows-x86_64') {
    Fail "combined manifest is missing a platform after merge (darwin/windows). NOT publishing."
  }
  OK "combined merge OK: platforms = $(($platforms.Keys) -join ', ') (version $combinedVersion)"
}

# ---- SHA256SUMS for the staged release files --------------------------------
$ReleaseFiles = @($Installer, $SigFile, $PerPlatformPath)
if ($MigrateLegacy -and $CombinedPath) { $ReleaseFiles += $CombinedPath }
$sumsPath = Join-Path $StageDir 'SHA256SUMS'
$ReleaseFiles | ForEach-Object {
  [pscustomobject]@{ name = (Split-Path $_ -Leaf); hash = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLower() }
} | Sort-Object name | ForEach-Object { "$($_.hash)  $($_.name)" } | Set-Content -LiteralPath $sumsPath -Encoding ASCII

# ---- Upload plan ------------------------------------------------------------
Step "Upload plan -> $Remote : $RemoteRoot"
$relList = "$SetupName, $SetupName.sig, $PerPlatformName, SHA256SUMS"
if ($MigrateLegacy) { $relList += ", latest.json" }
Write-Host "  releases/$Version/  <= $relList"
Write-Host "  updates/windows/$Channel.json  <= $PerPlatformName"
if ($MigrateLegacy) { Write-Host "  updates/latest.json  <= latest.json (combined, darwin preserved + windows added)" }
Write-Host "  prune: keep newest $KeepReleases releases"

if ($DryRun) {
  Step "DRY RUN - nothing uploaded. Staged files:"
  Get-ChildItem $StageDir | ForEach-Object { Write-Host "  $($_.FullName)" }
  Write-Host "  installer: $Installer"
  OK "dry run complete. Re-run without -DryRun to publish."
  return
}

# ---- Upload (OpenSSH ssh/scp; assumes key-based auth to the VM) -------------
Require-Command 'ssh' 'OpenSSH client required (Windows: Add-WindowsCapability OpenSSH.Client)'
Require-Command 'scp' 'OpenSSH client required'
$sshOpts = @('-o', 'StrictHostKeyChecking=accept-new')

function Invoke-SSH($remoteCmd) {
  ssh @sshOpts $Remote $remoteCmd
  if ($LASTEXITCODE -ne 0) { Fail "ssh failed: $remoteCmd" }
}
function Invoke-SCP($localPath, $remotePath) {
  scp @sshOpts $localPath "${Remote}:${remotePath}"
  if ($LASTEXITCODE -ne 0) { Fail "scp failed: $localPath -> $remotePath" }
}

Step "Upload"
Invoke-SSH "mkdir -p '$RemoteRoot/releases/$Version' '$RemoteRoot/updates/windows'"
Invoke-SCP $Installer       "$RemoteRoot/releases/$Version/"
Invoke-SCP $SigFile         "$RemoteRoot/releases/$Version/"
Invoke-SCP $PerPlatformPath "$RemoteRoot/releases/$Version/$PerPlatformName"
Invoke-SCP $sumsPath        "$RemoteRoot/releases/$Version/"
Invoke-SCP $PerPlatformPath "$RemoteRoot/updates/windows/$Channel.json"
OK "per-platform Windows manifest published (updates/windows/$Channel.json)"

if ($MigrateLegacy -and $CombinedPath) {
  Invoke-SCP $CombinedPath "$RemoteRoot/updates/latest.json"
  Invoke-SCP $CombinedPath "$RemoteRoot/releases/$Version/latest.json"
  OK "legacy combined manifest updated (windows-x86_64 merged, darwin preserved)"
}

# ---- Prune old releases (keep newest N by version) --------------------------
Step "Prune old releases (keep $KeepReleases)"
# Literal bash (no PowerShell interpolation of bash's own $vars); config arrives
# via the remote env prefix. Piped to ssh stdin (PowerShell has no '<<<').
$pruneScript = @'
set -euo pipefail
cd "$REMOTE_RELEASE_ROOT/releases"
mapfile -t versions < <(find . -mindepth 1 -maxdepth 1 -type d -print | sed 's#^\./##' | sort -V)
keep="$KEEP_RELEASES"
remove_count=$(( ${#versions[@]} - keep ))
if [ "$remove_count" -gt 0 ]; then
  for old in "${versions[@]:0:$remove_count}"; do echo "pruning $old"; rm -rf -- "$old"; done
fi
'@
$pruneScript | ssh @sshOpts $Remote "REMOTE_RELEASE_ROOT='$RemoteRoot' KEEP_RELEASES='$KeepReleases' bash -s"
if ($LASTEXITCODE -ne 0) { Warn "prune step returned nonzero (non-fatal)" }

# ---- Cleanup + done ---------------------------------------------------------
# Only reached on full success (DryRun returns early, Fail exits) - safe to drop
# the staging dir now; failures keep it around for troubleshooting.
if (Test-Path $StageDir) { Remove-Item -Recurse -Force $StageDir -ErrorAction SilentlyContinue }

Step "Done (v$Version)"
Write-Host "  per-platform: $PublicBaseUrl/updates/windows/$Channel.json"
Write-Host "  installer:    $PublicBaseUrl/releases/$Version/$SetupName"
if ($MigrateLegacy) { Write-Host "  combined:     $PublicBaseUrl/updates/latest.json (unsticks <=2.2.9 Windows users)" }
Write-Host ""
