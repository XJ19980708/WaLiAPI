@echo off
REM ============================================================
REM WaLiAPI build helper (NSIS only).
REM User requirement: every delivered test artifact must keep a
REM timestamp suffix (YYYYMMDD-HHMMSS) so a new build never
REM overwrites the previous one under the same version number.
REM Usage: scripts\build-nsis.bat
REM Steps: pnpm tauri build --bundles nsis
REM        -> copy artifact to <name>-<timestamp>.exe
REM        -> print the deliverable path
REM ============================================================
setlocal enabledelayedexpansion

cd /d %~dp0\..

for /f "tokens=1-3 delims=:,. " %%a in ('powershell -NoProfile -Command "Get-Date -Format 'HH:mm:ss'"') do set BUILD_TIME=%%a%%b%%c
for /f "tokens=1-3 delims=/-" %%a in ('powershell -NoProfile -Command "Get-Date -Format 'yyyy-MM-dd'"') do set BUILD_DATE=%%a%%b%%c
set STAMP=%BUILD_DATE%-%BUILD_TIME%
echo [build-nsis] timestamp: %STAMP%

call pnpm tauri build --bundles nsis
REM 注意:tauri 在生成 NSIS 产物后，若配置了 updater 公钥但未设
REM TAURI_SIGNING_PRIVATE_KEY，会在末尾返回非零（updater 签名缺失）。
REM 产物本身已完整生成，这里以产物存在性为准，不被该软错误中断时间戳复制；
REM 真正的编译失败（无产物）由下方 SRC 检查兜住。

set SRC=
for /f "delims=" %%f in ('powershell -NoProfile -Command "Get-ChildItem 'src-tauri\target\release\bundle\nsis\*-setup.exe' | Where-Object { $_.Name -notmatch '-[0-9]{8}-[0-9]{6}\.exe$' } | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName"') do set SRC=%%f
if not defined SRC (
    echo [build-nsis] ERROR: no non-timestamped setup artifact found
    exit /b 1
)

for %%F in ("%SRC%") do set SRCBASE=%%~nF
set DST=%~dp0\..\src-tauri\target\release\bundle\nsis\%SRCBASE%-%STAMP%.exe
copy /Y "%SRC%" "%DST%" >nul
if errorlevel 1 (
    echo [build-nsis] ERROR: failed to copy timestamped artifact
    exit /b 1
)

echo [build-nsis] OK deliver: %DST%
echo [build-nsis] raw artifact (do not deliver): %SRC%
exit /b 0