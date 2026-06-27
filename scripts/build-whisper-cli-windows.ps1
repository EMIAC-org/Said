param(
  [string]$Target = "x86_64-pc-windows-msvc"
)

# Build a self-contained whisper-cli.exe for AirNote's Windows bundle.
# Outputs:
#   target/<triple>/release/whisper-cli.exe
#   target/whisper-models/ggml-silero-v5.1.2.bin
#
# GPU acceleration (parity with the macOS build, which embeds Metal):
#   The build links the Vulkan backend (GGML_VULKAN=ON) when a Vulkan SDK is
#   available, so whisper runs on the GPU across vendors (NVIDIA / AMD / Intel)
#   and AUTOMATICALLY falls back to CPU when no compatible GPU or driver is
#   present. ggml chooses GPU-or-CPU at every launch — there is no persisted
#   setting. The Vulkan loader (vulkan-1.dll) ships with Windows 10 1803+, so a
#   Vulkan-enabled binary loads on every supported machine; no-GPU users simply
#   run on CPU exactly as before (no regression).
#
#   Control via env AIRNOTE_WHISPER_GPU:
#     auto (default) - use Vulkan if the SDK is found, otherwise CPU-only (warns)
#     vulkan         - require Vulkan; fail if the SDK is missing (use in CI/release)
#     cpu            - force a CPU-only build

$ErrorActionPreference = "Stop"

if ($Target -ne "x86_64-pc-windows-msvc") {
  throw "unsupported target: $Target (expected x86_64-pc-windows-msvc)"
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$WhisperRef = if ($env:WHISPER_CPP_REF) { $env:WHISPER_CPP_REF } else { "v1.7.6" }
$SrcDir = Join-Path $RepoRoot "target/whisper-cpp-src"
# Build under a SHORT path, not the repo's deep `target/`. The Vulkan backend
# compiles a nested `vulkan-shaders-gen` ExternalProject whose MSBuild .tlog
# paths overflow the Windows MAX_PATH (260 char) limit when the build tree sits
# under a long repo path (e.g. C:\Users\<name>\Documents\projects\...). Keeping
# the build tree under LOCALAPPDATA\aw keeps every intermediate path well clear
# of the limit. Outputs are still copied back into the repo's target/ below.
# Override with AIRNOTE_WHISPER_BUILD_DIR if needed.
$BuildDir = if ($env:AIRNOTE_WHISPER_BUILD_DIR) {
  $env:AIRNOTE_WHISPER_BUILD_DIR
} else {
  Join-Path $env:LOCALAPPDATA "aw\wcpp"
}
$OutDir = Join-Path $RepoRoot "target/$Target/release"
$OutBin = Join-Path $OutDir "whisper-cli.exe"
$ModelsDir = Join-Path $RepoRoot "target/whisper-models"
$SileroOut = Join-Path $ModelsDir "ggml-silero-v5.1.2.bin"

$GpuMode = if ($env:AIRNOTE_WHISPER_GPU) { $env:AIRNOTE_WHISPER_GPU.ToLower() } else { "auto" }

function Step($Message) {
  Write-Host ""
  Write-Host "==> $Message"
}

function Ok($Message) {
  Write-Host "  ok: $Message"
}

function Warn($Message) {
  Write-Host "  warning: $Message"
}

function Require-Command($Name) {
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "$Name not found"
  }
}

function Run-Checked([string]$Exe, [string[]]$ArgsForExe) {
  # Native tools (git, cmake) write normal progress/warnings to stderr. In
  # Windows PowerShell 5.1, with $ErrorActionPreference=Stop, that stderr is
  # promoted to a terminating error before we can inspect the exit code. Relax
  # it for the call and gate on the real exit code instead.
  $prev = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    & $Exe @ArgsForExe
    $code = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $prev
  }
  if ($code -ne 0) {
    throw "$Exe failed with exit code $code"
  }
}

# Locate a Vulkan SDK install: env VULKAN_SDK first, then the default
# C:\VulkanSDK\<version> layout (newest version wins).
function Find-VulkanSdk {
  if ($env:VULKAN_SDK -and (Test-Path (Join-Path $env:VULKAN_SDK "Include\vulkan\vulkan.h"))) {
    return $env:VULKAN_SDK
  }
  $root = "C:\VulkanSDK"
  if (Test-Path $root) {
    $latest = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
      Sort-Object Name -Descending | Select-Object -First 1
    if ($latest -and (Test-Path (Join-Path $latest.FullName "Include\vulkan\vulkan.h"))) {
      return $latest.FullName
    }
  }
  return $null
}

Require-Command "git"
Require-Command "cmake"

# Decide whether to build the Vulkan backend.
$UseVulkan = $false
$VulkanSdk = $null
if ($GpuMode -ne "cpu") {
  $VulkanSdk = Find-VulkanSdk
  if ($VulkanSdk) {
    $UseVulkan = $true
  } elseif ($GpuMode -eq "vulkan") {
    throw "AIRNOTE_WHISPER_GPU=vulkan but no Vulkan SDK found. Install it (winget install KhronosGroup.VulkanSDK) or set AIRNOTE_WHISPER_GPU=cpu."
  } else {
    Warn "Vulkan SDK not found - building CPU-only whisper-cli (no GPU acceleration). Install the Vulkan SDK for GPU support."
  }
}

if ($UseVulkan) {
  $env:VULKAN_SDK = $VulkanSdk
  # glslc (shader compiler) is needed by ggml's vulkan-shaders-gen at build time.
  $env:Path = (Join-Path $VulkanSdk "Bin") + ";" + $env:Path
  Ok "Vulkan SDK: $VulkanSdk"
}

if (-not (Test-Path (Join-Path $SrcDir ".git"))) {
  Step "Clone whisper.cpp"
  New-Item -ItemType Directory -Force -Path (Split-Path $SrcDir -Parent) | Out-Null
  Run-Checked "git" @(
    "clone",
    "--depth", "1",
    "--branch", $WhisperRef,
    "https://github.com/ggml-org/whisper.cpp",
    $SrcDir
  )
}

Step "Checkout whisper.cpp @ $WhisperRef"
# git writes normal progress to stderr; under $ErrorActionPreference=Stop,
# Windows PowerShell would promote that to a terminating error. Relax it just
# for these native calls, then check the real exit code.
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& git -C $SrcDir fetch --depth 1 origin $WhisperRef 2>&1 | Out-Null
& git -C $SrcDir checkout -q $WhisperRef 2>&1 | Out-Null
$checkoutOk = ($LASTEXITCODE -eq 0)
$ErrorActionPreference = $prevEap
if (-not $checkoutOk) {
  Run-Checked "git" @("-C", $SrcDir, "checkout", "-q", "FETCH_HEAD")
}
$ShortSha = (& git -C $SrcDir rev-parse --short HEAD).Trim()
Ok "source at $ShortSha"

$BackendLabel = if ($UseVulkan) { "Vulkan GPU + CPU fallback" } else { "CPU only" }
Step "Build whisper-cli.exe (x64, static, $BackendLabel)"
New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null
$CmakeConfigure = @(
  "-S", $SrcDir,
  "-B", $BuildDir,
  "-A", "x64",
  "-DCMAKE_BUILD_TYPE=Release",
  "-DBUILD_SHARED_LIBS=OFF",
  "-DGGML_NATIVE=OFF",
  "-DWHISPER_BUILD_TESTS=OFF",
  "-DWHISPER_BUILD_SERVER=OFF",
  "-DWHISPER_BUILD_EXAMPLES=ON"
)
if ($UseVulkan) {
  # Vulkan backend: GPU on NVIDIA/AMD/Intel, automatic CPU fallback at runtime.
  $CmakeConfigure += "-DGGML_VULKAN=ON"
}
Run-Checked "cmake" $CmakeConfigure
Run-Checked "cmake" @(
  "--build", $BuildDir,
  "--config", "Release",
  "--target", "whisper-cli",
  "--parallel"
)

$Built = Get-ChildItem -Path $BuildDir -Recurse -Filter "whisper-cli.exe" |
  Sort-Object FullName |
  Select-Object -First 1
if (-not $Built) {
  throw "whisper-cli.exe was not produced under $BuildDir"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Copy-Item $Built.FullName $OutBin -Force
Ok "whisper-cli.exe -> $OutBin ($BackendLabel)"

Step "Download Silero VAD model"
New-Item -ItemType Directory -Force -Path $ModelsDir | Out-Null
if (-not (Test-Path $SileroOut)) {
  Invoke-WebRequest `
    -Uri "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin" `
    -OutFile $SileroOut
}
if (-not (Test-Path $SileroOut)) {
  throw "Silero model missing at $SileroOut"
}
Ok "Silero VAD -> $SileroOut"

Ok "done: whisper-cli.exe ($BackendLabel) + Silero ready for $Target"
