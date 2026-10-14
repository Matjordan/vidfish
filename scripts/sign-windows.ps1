param([Parameter(Mandatory=$true)][string[]]$Files)
$ErrorActionPreference = 'Stop'
if (-not $env:VIDFISH_SIGN_CERT_BASE64 -and -not $env:VIDFISH_SIGN_CERT_PASSWORD) {
    Write-Output 'Vidfish signing is not configured; these Vidfish artifacts are unsigned.'
    return
}
if (-not $env:VIDFISH_SIGN_CERT_BASE64 -or -not $env:VIDFISH_SIGN_CERT_PASSWORD) { throw 'Both signing certificate and password are required' }
$tool = Get-ChildItem "${env:ProgramFiles(x86)}/Windows Kits/10/bin/*/x64/signtool.exe" | Sort-Object FullName -Descending | Select-Object -First 1
if (-not $tool) { throw 'Windows signing tool is missing' }
$certificate = Join-Path $env:RUNNER_TEMP ([guid]::NewGuid().ToString() + '.pfx')
try {
    [IO.File]::WriteAllBytes($certificate, [Convert]::FromBase64String($env:VIDFISH_SIGN_CERT_BASE64))
    foreach ($file in $Files) {
        & $tool.FullName sign /fd SHA256 /td SHA256 /tr https://timestamp.digicert.com /f $certificate /p $env:VIDFISH_SIGN_CERT_PASSWORD $file
        if ($LASTEXITCODE -ne 0) { throw 'Vidfish signing failed' }
        if ((Get-AuthenticodeSignature $file).Status -ne 'Valid') { throw 'Signed Vidfish artifact verification failed' }
    }
} finally { if (Test-Path $certificate) { Remove-Item $certificate -Force } }
