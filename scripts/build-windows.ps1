# Build the full Said Windows app: said-backend sidecar → sidecar slot →
# Vite dist → cargo tauri build (msi + nsis).
#
# Usage:
#   pwsh scripts/build-windows.ps1           # debug-ish bundle, no signing
#   pwsh scripts/build-windows.ps1 -Release  # release profile + signing
#
# Environment (for -Release):
#   WINDOWS_PFX_BASE64           — base64-encoded .pfx
#   WINDOWS_PFX_PASSWORD         — cert password
#   TAURI_SIGNING_PRIVATE_KEY    — Tauri updater EdDSA private key
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD — passphrase for above

[CmdletBinding()]
param(
    [switch]$Release,
    [string]$Target = 'x86_64-pc-windows-msvc'
)

$ErrorActionPreference = 'Stop'

$repo = (Get-Item "$PSScriptRoot/..").FullName
Push-Location $repo

try {
    $profile = if ($Release) { 'release' } else { 'debug' }
    $profileArg = if ($Release) { '--release' } else { '' }

    Write-Host "==> cargo build -p said-backend $profileArg --target $Target"
    if ($Release) {
        cargo build -p said-backend --release --target $Target
    } else {
        cargo build -p said-backend --target $Target
    }
    if ($LASTEXITCODE -ne 0) { throw "said-backend build failed" }

    $sidecarSrc = "target/$Target/$profile/said-backend.exe"
    $sidecarDst = "desktop/src-tauri/binaries/said-backend-$Target.exe"
    Write-Host "==> Staging sidecar $sidecarSrc -> $sidecarDst"
    New-Item -ItemType Directory -Force -Path 'desktop/src-tauri/binaries' | Out-Null
    Copy-Item -Force $sidecarSrc $sidecarDst

    Write-Host '==> npm ci + npm run build (Vite dist)'
    Push-Location desktop
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed' }
        npm run build
        if ($LASTEXITCODE -ne 0) { throw 'npm run build failed' }
    } finally {
        Pop-Location
    }

    Write-Host "==> cargo tauri build --bundles msi,nsis --target $Target"
    Push-Location desktop
    try {
        if ($Release) {
            npx tauri build --bundles msi,nsis --target $Target
        } else {
            npx tauri build --bundles msi,nsis --target $Target --debug
        }
        if ($LASTEXITCODE -ne 0) { throw 'tauri build failed' }
    } finally {
        Pop-Location
    }

    $bundleRoot = "desktop/src-tauri/target/$Target/$profile/bundle"
    $artifacts = @()
    if (Test-Path "$bundleRoot/msi") {
        $artifacts += Get-ChildItem -Recurse -Filter '*.msi' "$bundleRoot/msi" | ForEach-Object FullName
    }
    if (Test-Path "$bundleRoot/nsis") {
        $artifacts += Get-ChildItem -Recurse -Filter '*-setup.exe' "$bundleRoot/nsis" | ForEach-Object FullName
    }

    if ($Release) {
        if ($artifacts.Count -eq 0) {
            throw "No artifacts found under $bundleRoot"
        }
        Write-Host "==> Signing $($artifacts.Count) artifact(s)"
        pwsh "$PSScriptRoot/sign-windows.ps1" -Paths $artifacts
    }

    Write-Host "`nBuild complete. Artifacts:"
    foreach ($a in $artifacts) { Write-Host "  $a" }
} finally {
    Pop-Location
}
