# Official MSVC runtime + development SDK. HTTPS verification remains enabled.
param([ValidateSet('1.26.10')][string]$Version = '1.26.10')
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = 'C:\gstreamer\1.0\msvc_x86_64'
$temporary = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$cache = Join-Path $temporary 'vidfish-gstreamer'
New-Item -ItemType Directory -Force $cache | Out-Null
try {
foreach ($kind in @('', '-devel')) {
    $name = "gstreamer-1.0$kind-msvc-x86_64-$Version.msi"
    $file = Join-Path $cache $name
    if (-not (Test-Path $file)) { Invoke-WebRequest "https://gstreamer.freedesktop.org/data/pkg/windows/$Version/msvc/$name" -OutFile $file }
    # Pinned from the official installers' .sha256sum files; verify cached files too.
    $expected = if ($kind -eq '') { 'A863BF3FAA49E9F33BD3CC42967B473482D4DC98655ED95CBA1AC59F26FB0CFB' } else { '8487A115FEA3B0B0B4C55A9A413AD1ADAB0FE78F6681FEEDC60858A48613A121' }
    if ((Get-FileHash $file -Algorithm SHA256).Hash -ne $expected) { throw "GStreamer installer checksum mismatch: $name" }
    $process = Start-Process msiexec.exe -Wait -PassThru -ArgumentList @('/i', $file, '/qn', '/norestart', 'ADDLOCAL=ALL', "INSTALLDIR=$root")
    if ($process.ExitCode -notin @(0, 3010)) { throw "GStreamer installer failed: $($process.ExitCode)" }
}
if (-not (Test-Path "$root\bin\gst-inspect-1.0.exe")) {
    # Recent installers use Program Files; older releases used C:\gstreamer.
    $locations = @("$env:ProgramFiles\gstreamer", "${env:ProgramFiles(x86)}\gstreamer", 'C:\gstreamer')
    $binary = $locations | Where-Object { Test-Path $_ } | ForEach-Object {
        Get-ChildItem $_ -Filter gst-inspect-1.0.exe -Recurse -ErrorAction SilentlyContinue
    } | Select-Object -First 1
    if (-not $binary) { throw 'GStreamer runtime not found in known installer locations' }
    $root = Split-Path (Split-Path $binary.FullName)
}
& "$root\bin\gst-inspect-1.0.exe" --version
if ($LASTEXITCODE -ne 0) { throw 'Installed GStreamer runtime could not launch' }
if (-not (Test-Path "$root\bin\pkg-config.exe")) {
    choco install pkgconfiglite --yes --no-progress
    if ($LASTEXITCODE -ne 0) { throw 'pkg-config installation failed' }
}
$env:PATH = "$root\bin;$env:PATH"
$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $root
$env:PKG_CONFIG_PATH = "$root\lib\pkgconfig"
if ($env:GITHUB_PATH) { "$root\bin" | Out-File -FilePath $env:GITHUB_PATH -Encoding utf8 -Append }
if ($env:GITHUB_ENV) {
    "GSTREAMER_1_0_ROOT_MSVC_X86_64=$root" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
    "PKG_CONFIG_PATH=$root\lib\pkgconfig" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
} catch {
    # Emit a GitHub annotation as well as the log, so install failures are visible
    # through the checks API when archived log storage is unavailable.
    $message = $_.Exception.Message.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
    Write-Output "::error title=GStreamer setup failed::$message"
    throw
}
