@echo off
REM Build the isolated GPU worker (asr-core + vulkan). Needs the same Ninja +
REM short-target recipe as the old app Vulkan build (ggml-vulkan shader-gen
REM overflows MAX_PATH under MSBuild / long paths).
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
set "PATH=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja;%PATH%"
set CMAKE_GENERATOR=Ninja
set VULKAN_SDK=C:\VulkanSDK\1.4.350.0
set CARGO_TARGET_DIR=C:\stw
REM Deterministic ISA: never bake the build host's AVX512 into ggml (kills
REM 12th-gen+ CPUs with illegal instruction). AVX2 floor, same as prod script.
set GGML_NATIVE=OFF
set GGML_AVX2=ON
cd /d C:\Users\anish\Documents\projects\said\Said\crates\asr-gpu-worker
echo === building airnote-asr-gpu (CARGO_TARGET_DIR=%CARGO_TARGET_DIR%) ===
cargo build
echo CARGO_EXIT=%ERRORLEVEL%
