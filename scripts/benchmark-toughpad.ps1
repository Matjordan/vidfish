# Run on the actual target; synthetic-source/cloud results do not satisfy #8.
param(
    [string]$Binary = '.\vidfish.exe',
    [Parameter(Mandatory=$true)][string[]]$CameraUris,
    [ValidateRange(10, 3600)][int]$Seconds = 60,
    [string]$OutputDirectory = '.\toughpad-measurement'
)
$ErrorActionPreference = 'Stop'
if ($CameraUris.Count -ne 4) { throw 'Supply exactly four real 1080p camera profile URIs' }
foreach ($uri in $CameraUris) {
    if ($uri -notmatch '^rtsps?://' -or $uri -match '[\s"]') { throw 'Supply RTSP URIs with URL-encoded special characters' }
}
if (Test-Path $OutputDirectory) { throw 'Choose a new measurement directory; existing results are preserved' }
$binaryPath = (Resolve-Path $Binary).Path
New-Item -ItemType Directory $OutputDirectory | Out-Null
$output = (Resolve-Path $OutputDirectory).Path
$computer = Get-CimInstance Win32_ComputerSystem
$os = Get-CimInstance Win32_OperatingSystem
$processors = @(Get-CimInstance Win32_Processor)
$gpus = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, AdapterRAM)
$logicalProcessors = [Environment]::ProcessorCount
$arguments = @('--desktop-seconds', "$Seconds")
foreach ($uri in $CameraUris) { $arguments += @('--preview', $uri) }
$previousLogFilter = $env:RUST_LOG
$env:RUST_LOG = 'info'
$process = Start-Process -FilePath $binaryPath -ArgumentList $arguments -PassThru -RedirectStandardOutput "$output/preview.jsonl" -RedirectStandardError "$output/errors.log"
$clock = [System.Diagnostics.Stopwatch]::StartNew()
$samples = @()
$gpuStatus = 'available'
$lastCpu = 0.0
$lastElapsed = 0.0
try {
    while (-not $process.HasExited) {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) { break }
        $elapsed = $clock.Elapsed.TotalSeconds
        $cpu = $process.TotalProcessorTime.TotalSeconds
        $cpuPercent = 100.0 * ($cpu - $lastCpu) / ($elapsed - $lastElapsed)
        $gpu = @()
        if ($gpuStatus -eq 'available') {
            try {
                $gpu = @((Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples |
                    Where-Object { $_.Path -match "pid_$($process.Id)_" } |
                    Select-Object Path, CookedValue)
            } catch { $gpuStatus = 'unavailable: counter not supported, inaccessible, or localized; measure with a GPU vendor tool' }
        }
        $samples += [pscustomobject]@{ elapsed_seconds=$elapsed; cpu_one_core_percent=$cpuPercent;
            cpu_system_capacity_percent=($cpuPercent / $logicalProcessors); working_set_bytes=$process.WorkingSet64; gpu_engines=$gpu }
        $lastCpu=$cpu; $lastElapsed=$elapsed
        if ($elapsed -gt $Seconds + 30) { throw 'Preview process exceeded benchmark timeout' }
    }
    $process.WaitForExit()
    $logs = @(Get-Content "$output/preview.jsonl" | ForEach-Object { try { $_ | ConvertFrom-Json } catch {} })
    $frames = @($logs | Where-Object { $_.fields.message -eq 'preview measurement' } | ForEach-Object { $_.fields })
    $decoders = @($logs | Where-Object { $_.fields.message -eq 'preview decoder selected' } | ForEach-Object { $_.fields.decoder })
    $renderer = @($logs | Where-Object { $_.fields.message -eq 'desktop renderer initialized' } | ForEach-Object { $_.fields.renderer })
    $profiles = @($logs | Where-Object { $_.fields.message -eq 'preview negotiated' } | ForEach-Object { $_.fields })
    $result = [pscustomobject]@{
        model=$computer.Model; manufacturer=$computer.Manufacturer; os=$os.Caption; os_version=$os.Version;
        cpu=@($processors | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors); gpu=$gpus;
        app_version=(& $binaryPath --version); camera_count=4; expected_resolution='1920x1080'; duration_seconds=$Seconds;
        exit_code=$process.ExitCode; renderer=$renderer; decoders=$decoders; frames=$frames;
        negotiated_profiles=$profiles;
        gpu_counter_status=$gpuStatus; samples=$samples;
        acceptance='Review actual profile resolution/fps/bitrate and CPU/GPU measurements; unavailable GPU counters require separate measurements.'
    }
    $result | ConvertTo-Json -Depth 12 | Set-Content "$output/result.json" -Encoding UTF8
    if ($process.ExitCode -ne 0 -or $frames.Count -ne 4 -or $profiles.Count -ne 4 -or @($profiles | Where-Object { $_.width -ne 1920 -or $_.height -ne 1080 }).Count -ne 0) { throw "Four-1080p preview benchmark failed; inspect $output" }
    Write-Output "Measurements saved to $output/result.json. Credentials/URIs are not included."
} finally {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    $env:RUST_LOG = $previousLogFilter
}
