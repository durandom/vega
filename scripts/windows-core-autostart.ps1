#requires -Version 5.1
# Register only: never starts or stops the current core.
[CmdletBinding()]
param([string]$HcPath = (Join-Path $env:LOCALAPPDATA 'Programs\herdr-core\hc.exe'))
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$HcPath = [IO.Path]::GetFullPath($HcPath)
if (-not (Test-Path -LiteralPath $HcPath -PathType Leaf)) { throw "hc executable not found: $HcPath" }
$directory = Join-Path $env:LOCALAPPDATA 'vega\core-autostart'
New-Item -ItemType Directory -Force -Path $directory | Out-Null
$launcher = Join-Path $directory 'logon.vbs'
# Never elevate or repair an inaccessible core. hc server start reuses a
# running core and serializes core creation with its existing lock.
$escapedHc = $HcPath.Replace("'", "''")
$body = @"
`$ErrorActionPreference = 'Stop'
if ([Diagnostics.Process]::GetCurrentProcess().SessionId -eq 0) { exit 1 }
`$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
`$principal = New-Object Security.Principal.WindowsPrincipal(`$identity)
if (`$principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { exit 1 }
& '$escapedHc' server start *> (Join-Path `$env:LOCALAPPDATA 'vega\core-autostart\last-start.log')
exit `$LASTEXITCODE
"@
$encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($body))
$powershell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$command = '"' + $powershell + '" -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand ' + $encoded
$vbs = 'Set shell = CreateObject("WScript.Shell")' + "`r`n" +
    'result = shell.Run("' + $command.Replace('"', '""') + '", 0, True)' + "`r`n" +
    'WScript.Quit result' + "`r`n"
Set-Content -LiteralPath $launcher -Value $vbs -Encoding Unicode -NoNewline
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (-not (Test-Path $run)) { New-Item -Path $run | Out-Null }
$value = '"' + (Join-Path $env:SystemRoot 'System32\wscript.exe') + '" "' + $launcher + '"'
New-ItemProperty -Path $run -Name 'VegaHcDesktopCore' -PropertyType String -Value $value -Force | Out-Null
Write-Output "core_autostart: registered HKCU Run VegaHcDesktopCore -> $launcher (next desktop logon; no core started)"
