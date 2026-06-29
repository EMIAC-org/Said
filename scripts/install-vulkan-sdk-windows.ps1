<#
.SYNOPSIS
  Install the Vulkan SDK needed to build AirNote's Windows whisper-cli with GPU
  acceleration.

.DESCRIPTION
  GitHub's windows-latest runner does not include the Vulkan SDK. Without it,
  scripts/build-whisper-cli-windows.ps1 falls back to CPU-only whisper-cli in
  auto mode. Release workflows use this script plus AIRNOTE_WHISPER_GPU=vulkan
  so a missing SDK fails the build instead of shipping a slow local STT binary.

  The script is intentionally repo-owned rather than a third-party action:
  - pinned SDK version
  - verifies vulkan.h and glslc.exe
  - exports VULKAN_SDK and PATH for later GitHub Actions steps
#>
[CmdletBinding()]
param(
  [string]$Version = "1.3.290.0"
)

$ErrorActionPreference = "Stop"

function Step($m) { Write-Host "`n> $m" -ForegroundColor White }
function OK($m)   { Write-Host "  + $m"  -ForegroundColor Green }
function Fail($m) { Write-Host "`n  x $m`n" -ForegroundColor Red; exit 1 }

$SdkDir = "C:\VulkanSDK\$Version"
$Header = Join-Path $SdkDir "Include\vulkan\vulkan.h"
$Glslc = Join-Path $SdkDir "Bin\glslc.exe"
$InstallerUrl = "https://sdk.lunarg.com/sdk/download/$Version/windows/VulkanSDK-$Version-Installer.exe"
$Installer = Join-Path $env:RUNNER_TEMP "VulkanSDK-$Version-Installer.exe"
if (-not $env:RUNNER_TEMP) {
  $Installer = Join-Path ([System.IO.Path]::GetTempPath()) "VulkanSDK-$Version-Installer.exe"
}

if ((Test-Path $Header) -and (Test-Path $Glslc)) {
  Step "Vulkan SDK already installed"
  OK "$SdkDir"
} else {
  Step "Download Vulkan SDK $Version"
  Invoke-WebRequest -Uri $InstallerUrl -OutFile $Installer
  if (-not (Test-Path $Installer)) {
    Fail "installer download failed: $InstallerUrl"
  }

  Step "Install Vulkan SDK $Version"
  # LunarG's installer is built with the Qt Installer Framework, NOT NSIS, so it
  # does not understand "/S" - it silently ignores the flag and opens the
  # interactive GUI, which hangs forever on a headless CI runner. Use QtIFW's
  # unattended flags instead, and cap the wait so any future GUI-hang fails fast
  # rather than stalling the job until the workflow timeout.
  $installArgs = @("--accept-licenses", "--default-answer", "--confirm-command", "install")
  $proc = Start-Process -FilePath $Installer -ArgumentList $installArgs -PassThru
  if (-not $proc.WaitForExit(900000)) {
    try { $proc.Kill() } catch {}
    Fail "Vulkan SDK installer timed out (>15 min) - likely waiting on a GUI prompt; verify the unattended flags."
  }
  if ($proc.ExitCode -ne 0) {
    Fail "Vulkan SDK installer exited with $($proc.ExitCode)"
  }
}

if (-not (Test-Path $Header)) {
  Fail "Vulkan header missing after install: $Header"
}
if (-not (Test-Path $Glslc)) {
  Fail "glslc missing after install: $Glslc"
}

$env:VULKAN_SDK = $SdkDir
$env:Path = (Join-Path $SdkDir "Bin") + ";" + $env:Path

if ($env:GITHUB_ENV) {
  "VULKAN_SDK=$SdkDir" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
}
if ($env:GITHUB_PATH) {
  Join-Path $SdkDir "Bin" | Out-File -FilePath $env:GITHUB_PATH -Append -Encoding utf8
}

Step "Verify Vulkan SDK"
& $Glslc --version
if ($LASTEXITCODE -ne 0) {
  Fail "glslc verification failed"
}
OK "Vulkan SDK ready: $SdkDir"
