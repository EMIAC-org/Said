@echo off
REM Wrapper: load MSVC x64 env (cl.exe + Ninja on PATH) then run the Windows
REM production build. Windows dictation is HYBRID (dictation_stt.rs): Auto/
REM On-device/Hosted selectable in Settings — the airnote-asr-gpu worker ships
REM in the bundle (reused from binaries\, -SkipWorker only skips REBUILDING it;
REM use -RebuildWorker after asr-core/worker changes). Backend reused too.
REM DEEPINFRA_API_KEY + DEEPSEEK_API_KEY are baked from repo-root .env.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d C:\Users\anish\Documents\projects\said\Said
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\build-windows.ps1" -SkipBackend -SkipWorker -RequireInstaller
echo PROD_BUILD_EXIT=%ERRORLEVEL%
