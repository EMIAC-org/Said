@echo off
REM Wrapper: load MSVC x64 env (cl.exe + Ninja on PATH) then run the Windows
REM production build. Windows dictation is fixed to live Together Nemotron;
REM the on-device worker still ships for meetings. It is reused from binaries\,
REM -SkipWorker only skips REBUILDING it; use -RebuildWorker after asr-core/
REM worker changes. Backend reused too.
REM TOGETHER_API_KEY + DEEPSEEK_API_KEY are baked from repo-root .env.
call "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
cd /d C:\Users\anish\Documents\projects\said\Said
powershell -NoProfile -ExecutionPolicy Bypass -File "scripts\build-windows.ps1" -SkipBackend -SkipWorker -RequireInstaller
echo PROD_BUILD_EXIT=%ERRORLEVEL%
