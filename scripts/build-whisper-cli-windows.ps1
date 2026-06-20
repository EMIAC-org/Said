param(
  [string]$Target = "x86_64-pc-windows-msvc"
)

# Build a self-contained whisper-cli.exe for AirNote's Windows bundle.
# Outputs:
#   target/<triple>/release/whisper-cli.exe
#   target/whisper-models/ggml-silero-v5.1.2.bin

$ErrorActionPreference = "Stop"

if ($Target -ne "x86_64-pc-windows-msvc") {
  throw "unsupported target: $Target (expected x86_64-pc-windows-msvc)"
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$WhisperRef = if ($env:WHISPER_CPP_REF) { $env:WHISPER_CPP_REF } else { "v1.7.6" }
$SrcDir = Join-Path $RepoRoot "target/whisper-cpp-src"
$BuildDir = Join-Path $RepoRoot "target/whisper-cpp-build/$Target"
$OutDir = Join-Path $RepoRoot "target/$Target/release"
$OutBin = Join-Path $OutDir "whisper-cli.exe"
$ModelsDir = Join-Path $RepoRoot "target/whisper-models"
$SileroOut = Join-Path $ModelsDir "ggml-silero-v5.1.2.bin"

function Step($Message) {
  Write-Host ""
  Write-Host "==> $Message"
}

function Ok($Message) {
  Write-Host "  ok: $Message"
}

function Require-Command($Name) {
  if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "$Name not found"
  }
}

function Run-Checked([string]$Exe, [string[]]$ArgsForExe) {
  & $Exe @ArgsForExe
  if ($LASTEXITCODE -ne 0) {
    throw "$Exe failed with exit code $LASTEXITCODE"
  }
}

Require-Command "git"
Require-Command "cmake"

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
& git -C $SrcDir fetch --depth 1 origin $WhisperRef 2>$null
& git -C $SrcDir checkout -q $WhisperRef 2>$null
if ($LASTEXITCODE -ne 0) {
  Run-Checked "git" @("-C", $SrcDir, "checkout", "-q", "FETCH_HEAD")
}
$ShortSha = (& git -C $SrcDir rev-parse --short HEAD).Trim()
Ok "source at $ShortSha"

Step "Build whisper-cli.exe (x64, static)"
New-Item -ItemType Directory -Force -Path $BuildDir | Out-Null
Run-Checked "cmake" @(
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
Ok "whisper-cli.exe -> $OutBin"

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

Ok "done: whisper-cli.exe + Silero ready for $Target"
