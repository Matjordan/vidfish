@echo off
setlocal
set "PATH=%~dp0gstreamer\bin;%PATH%"
set "GST_PLUGIN_SYSTEM_PATH_1_0=%~dp0gstreamer\lib\gstreamer-1.0"
if exist "%~dp0gstreamer\libexec\gstreamer-1.0\gst-plugin-scanner.exe" set "GST_PLUGIN_SCANNER_1_0=%~dp0gstreamer\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
"%~dp0vidfish.exe" --media-check >nul
if errorlevel 1 (
    echo Runtime check failed. Install the included Microsoft VC runtime and see docs\media.md.
    exit /b 1
)
"%~dp0vidfish.exe" %*
exit /b %errorlevel%
