# Official MSVC runtime + development SDK. HTTPS verification remains enabled.
param([string]$Version = '1.26.2')
$ErrorActionPreference = 'Stop'
$root = 'C:\gstreamer\1.0\msvc_x86_64'
$cache = Join-Path $env:RUNNER_TEMP 'vidfish-gstreamer'
New-Item -ItemType Directory -Force $cache | Out-Null
try {
foreach ($kind in @('', '-devel')) {
    $name = "gstreamer-1.0$kind-msvc-x86_64-$Version.msi"
    $file = Join-Path $cache $name
    Invoke-WebRequest "https://gstreamer.freedesktop.org/data/pkg/windows/$Version/msvc/$name" -OutFile $file
    $process = Start-Process msiexec.exe -Wait -PassThru -ArgumentList @('/i', $file, '/qn', '/norestart', 'ADDLOCAL=ALL')
    if ($process.ExitCode -notin @(0, 3010)) { throw "GStreamer installer failed: $($process.ExitCode)" }
}
if (-not (Test-Path "$root\bin\gst-inspect-1.0.exe")) { throw 'GStreamer runtime not found' }
"$root\bin" | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append
"GSTREAMER_1_0_ROOT_MSVC_X86_64=$root" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
"PKG_CONFIG_PATH=$root\lib\pkgconfig" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
} catch {
    # Emit a GitHub annotation as well as the log, so install failures are visible
    # through the checks API when archived log storage is unavailable.
    $message = $_.Exception.Message.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
    Write-Output "::error title=GStreamer setup failed::$message"
    throw
}
