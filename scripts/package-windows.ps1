$ErrorActionPreference = 'Stop'
if ($env:VIDFISH_VERSION -notmatch '^[0-9A-Za-z.+-]+$') { throw 'Invalid version' }
$root = "dist/windows/vidfish-$env:VIDFISH_VERSION"
New-Item -ItemType Directory -Force "$root/docs", "$root/gstreamer", "$root/assets" | Out-Null
Copy-Item target/release/vidfish.exe, target/release/vidfish-maintenance.exe, README.md, scripts/launch-vidfish.cmd, scripts/benchmark-toughpad.ps1 $root/
Copy-Item docs/media.md, docs/cameras.md, docs/recording.md, docs/monitoring.md, docs/events.md, docs/sessions.md "$root/docs/"
Copy-Item assets/vidfish.* "$root/assets/"
$gst = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64
if (-not $gst) { throw 'GStreamer runtime location missing' }
# Keep runtime DLLs, plugins, scanner, presets, and original notices together.
Copy-Item "$gst/bin", "$gst/share" "$root/gstreamer/" -Recurse
New-Item -ItemType Directory -Force "$root/gstreamer/lib" | Out-Null
Copy-Item "$gst/lib/gstreamer-1.0" "$root/gstreamer/lib/" -Recurse
if (Test-Path "$gst/libexec") { Copy-Item "$gst/libexec" "$root/gstreamer/" -Recurse }
# Distribute Microsoft's signed redistributable; users never need Visual Studio.
Invoke-WebRequest 'https://aka.ms/vs/17/release/vc_redist.x64.exe' -OutFile "$root/vc_redist.x64.exe"
$signature = Get-AuthenticodeSignature "$root/vc_redist.x64.exe"
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft Corporation') { throw 'Microsoft redistributable signature verification failed' }
$version = & "$root/vidfish.exe" --version
if ($version -ne "vidfish $env:VIDFISH_VERSION") { throw 'Version mismatch' }
# Use only the bundled GStreamer path for package readiness, not the build SDK.
$oldPath = $env:PATH
$oldPlugins = $env:GST_PLUGIN_SYSTEM_PATH_1_0
try {
    $env:PATH = "$env:WINDIR/System32;$env:WINDIR"
    $env:GST_PLUGIN_SYSTEM_PATH_1_0 = "$((Resolve-Path $root).Path)/gstreamer/lib/gstreamer-1.0"
    & "$root/launch-vidfish.cmd" --check
    if ($LASTEXITCODE -ne 0) { throw 'Packaged runtime launch failed' }
} finally { $env:PATH = $oldPath; $env:GST_PLUGIN_SYSTEM_PATH_1_0 = $oldPlugins }
python scripts/make-update-payload.py $root $env:VIDFISH_VERSION windows-x86_64
if ($LASTEXITCODE -ne 0) { throw "Update payload generation failed" }
Compress-Archive -Path $root -DestinationPath "vidfish-$env:VIDFISH_VERSION-windows-x86_64.zip"

& "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe" "/DReleaseVersion=$env:VIDFISH_VERSION" "/DBundleRoot=$((Resolve-Path $root).Path)" scripts/vidfish-installer.iss
if ($LASTEXITCODE -ne 0) { throw "Native installer compilation failed" }
