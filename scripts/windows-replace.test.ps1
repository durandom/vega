#requires -Version 5.1
param([string]$Post = (Join-Path $PSScriptRoot '..\target\debug\vega-post.exe'))
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'windows-replace.ps1')
$Post = (Resolve-Path -LiteralPath $Post).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ("vega image test ' " + [guid]::NewGuid())
$saved = @{}
$exe = $null
foreach ($name in @('XDG_STATE_HOME', 'XDG_CONFIG_HOME', 'VEGA_NATS_URL')) {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
try {
    $env:XDG_STATE_HOME = Join-Path $root 'state'
    $env:XDG_CONFIG_HOME = Join-Path $root 'config'
    $env:VEGA_NATS_URL = 'nats://127.0.0.1:1'
    $install = Join-Path $root 'install'
    $images = @([pscustomobject]@{ Source=$Post; Name='vega-post.exe' })
    Install-VegaImages $images $install
    $exe = Join-Path $install 'vega-post.exe'
    & $exe relay start
    if ($LASTEXITCODE -ne 0) { throw 'Fixture worker did not start.' }
    $before = & $exe relay status --json | ConvertFrom-Json
    if (-not $before.running) { throw 'Fixture worker did not survive start.' }
    Install-VegaImages $images $install
    $after = & $exe relay status --json | ConvertFrom-Json
    if (-not $after.running -or $after.pid -eq $before.pid) { throw 'Update did not restart mapped image.' }
    if ((Get-FileHash -LiteralPath $exe).Hash -ne (Get-FileHash -LiteralPath $Post).Hash) { throw 'Installed bytes differ.' }
    if (Get-ChildItem -LiteralPath $install -Filter '*.old-*') { throw 'Stopped image was not cleaned up.' }
    # Staging failure keeps the current worker and installed image intact.
    $bad = @($images[0], [pscustomobject]@{ Source=(Join-Path $root 'missing.exe'); Name='missing.exe' })
    $failed = $false
    try { Install-VegaImages $bad $install } catch { $failed = $true }
    if (-not $failed) { throw 'Missing source did not fail.' }
    $still = & $exe relay status --json | ConvertFrom-Json
    if (-not $still.running -or $still.pid -ne $after.pid) { throw 'Staging failure changed the running worker.' }
    & $exe relay stop
    if ($LASTEXITCODE -ne 0) { throw 'Fixture stop failed.' }
    Write-Output 'Windows mapped-image update/restart and staging failure: passed.'
} finally {
    if ($exe -and (Test-Path -LiteralPath $exe)) { & $exe relay stop }
    foreach ($name in $saved.Keys) { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
