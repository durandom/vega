#requires -Version 5.1
# Run on the disposable Windows CI account; never execute the logon launcher.
$ErrorActionPreference = 'Stop'
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$name = 'VegaHcDesktopCore'
$directory = Join-Path $env:LOCALAPPDATA 'vega\core-autostart'
if ((Get-ItemProperty -Path $run -Name $name -ErrorAction SilentlyContinue) -or (Test-Path $directory)) {
    throw 'Autostart test requires a disposable account without an existing registration.'
}
$temporary = Join-Path $env:TEMP ('vega-core-test-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $temporary | Out-Null
$marker = Join-Path $temporary 'started'
$fake = Join-Path $temporary 'hc.cmd'
Set-Content $fake "@echo off`r`necho started > `"$marker`""
try {
    $installer = Join-Path $PSScriptRoot 'windows-install.ps1'
    & powershell.exe -NoProfile -File $installer -CoreAutostartOnly -HcPath $fake
    if ($LASTEXITCODE -ne 0) { throw 'Registration failed.' }
    $first = Get-ItemPropertyValue $run $name
    $launcher = Get-Content -Raw (Join-Path $directory 'logon.vbs')
    & powershell.exe -NoProfile -File $installer -CoreAutostartOnly -HcPath $fake
    if ($LASTEXITCODE -ne 0) { throw 'Repeat registration failed.' }
    if ($first -cne (Get-ItemPropertyValue $run $name)) { throw 'Run value is not idempotent.' }
    if ($launcher -cne (Get-Content -Raw (Join-Path $directory 'logon.vbs'))) { throw 'Launcher is not idempotent.' }
    if (Test-Path $marker) { throw 'Registration started hc.' }
    # Validate the WScript wrapper without executing hc or its logon payload.
    $safePayload = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('exit 0'))
    $safeLauncher = $launcher -replace '(-EncodedCommand )[A-Za-z0-9+/=]+', ('$1' + $safePayload)
    $safePath = Join-Path $temporary 'syntax.vbs'
    Set-Content $safePath $safeLauncher -Encoding Unicode
    & cscript.exe //B //NoLogo $safePath
    if ($LASTEXITCODE -ne 0) { throw 'WScript launcher failed.' }
    $rejected = $false
    try {
        & (Join-Path $PSScriptRoot 'windows-core-autostart.ps1') -HcPath (Join-Path $temporary 'missing.exe')
    } catch { $rejected = $true }
    if (-not $rejected) { throw 'Missing hc should fail.' }
    if ($first -cne (Get-ItemPropertyValue $run $name)) { throw 'Failed registration changed the Run value.' }
    Write-Output 'core autostart: idempotent, registration-only, missing executable rejected'
} finally {
    Remove-ItemProperty $run $name -ErrorAction SilentlyContinue
    Remove-Item $directory -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $temporary -Recurse -Force
}
