#requires -Version 5.1
<#
.SYNOPSIS
Build the Windows-ready vega subset natively and install for the current user.
.EXAMPLE
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\windows-install.ps1
.EXAMPLE
.\scripts\windows-install.ps1 -Checkout C:\Users\hild\src\vega -SkipUpdate
#>
[CmdletBinding()]
param(
    [string]$Checkout = (Join-Path $env:USERPROFILE 'src\vega'),
    [string]$Repository = 'https://github.com/durandom/vega.git',
    [string]$Ref = 'main',
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\vega'),
    [switch]$SkipUpdate,
    [switch]$NativeTls,
    [switch]$CoreAutostart,
    [switch]$CoreAutostartOnly,
    [string]$HcPath = (Join-Path $env:LOCALAPPDATA 'Programs\herdr-core\hc.exe')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Opt-in: mail clients can be installed without hc. Registration never starts it.
if ($CoreAutostartOnly) {
    & (Join-Path $PSScriptRoot 'windows-core-autostart.ps1') -HcPath $HcPath
    exit 0
}

function Invoke-Native {
    param([string]$Program, [string[]]$Arguments)
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program exited $LASTEXITCODE" }
}

function Add-PathEntry {
    param([string]$Value, [string]$Entry)
    $normalized = $Entry.TrimEnd('\')
    foreach ($part in ($Value -split ';')) {
        if ([Environment]::ExpandEnvironmentVariables($part).Trim().TrimEnd('\') -ieq $normalized) {
            return $Value
        }
    }
    if ([string]::IsNullOrEmpty($Value)) { return $Entry }
    return "$Entry;$Value"
}

$savedEnvironment = @{}
foreach ($name in @('CARGO_HOME', 'CARGO_TARGET_DIR', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTUP_USE_RUSTLS', 'Path', 'GIT_TERMINAL_PROMPT')) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$process = [Diagnostics.Process]::GetCurrentProcess()
$savedPriority = $process.PriorityClass
$savedLocation = Get-Location
try {
    $process.PriorityClass = 'BelowNormal'
    $env:GIT_TERMINAL_PROMPT = '0'
    if ($NativeTls) { $env:RUSTUP_USE_RUSTLS = '0' }
    $gitCommand = Get-Command git.exe -ErrorAction SilentlyContinue
    $git = if ($gitCommand) { $gitCommand.Source } else { Join-Path $env:ProgramFiles 'Git\cmd\git.exe' }
    if (-not (Test-Path -LiteralPath $git)) { throw 'Install Git for Windows in user space, then rerun.' }
    $Checkout = [IO.Path]::GetFullPath($Checkout)
    $InstallDir = [IO.Path]::GetFullPath($InstallDir)
    if (-not (Test-Path -LiteralPath $Checkout)) {
        if ($SkipUpdate) { throw '-SkipUpdate requires an existing checkout.' }
        New-Item -ItemType Directory -Force -Path (Split-Path $Checkout) | Out-Null
        Invoke-Native $git @('clone', '--branch', $Ref, '--single-branch', $Repository, $Checkout)
    } elseif (-not $SkipUpdate) {
        $dirty = & $git -C $Checkout status --porcelain
        if ($LASTEXITCODE -ne 0) { throw 'Checkout is not a Git repository.' }
        if ($dirty) { throw 'Checkout has local changes; commit them or use -SkipUpdate deliberately.' }
        $branch = & $git -C $Checkout symbolic-ref --short HEAD
        if ($LASTEXITCODE -ne 0 -or $branch -ne $Ref) { throw "Checkout must be on branch $Ref; select -Ref explicitly or use -SkipUpdate." }
        Invoke-Native $git @('-C', $Checkout, 'fetch', 'origin', $Ref)
        Invoke-Native $git @('-C', $Checkout, 'merge', '--ff-only', 'FETCH_HEAD')
    }
    Set-Location -LiteralPath $Checkout
    $packages = Get-Content -Raw -LiteralPath (Join-Path $Checkout 'scripts\windows-packages.json') | ConvertFrom-Json
    $packages = @($packages)
    if ($packages.Count -eq 0) { throw 'Windows package allowlist is empty.' }
    foreach ($package in $packages) {
        if ($package.package -notmatch '^[a-zA-Z0-9_-]+$') { throw 'Invalid package name in Windows allowlist.' }
        foreach ($binary in $package.binaries) {
            if ($binary -notmatch '^[a-zA-Z0-9_-]+$') { throw "Invalid binary name: $binary" }
        }
    }
    $pin = Get-Content -Raw -LiteralPath (Join-Path $Checkout 'rust-toolchain.toml')
    if ($pin -notmatch '(?m)^channel\s*=\s*"([^"]+)"') { throw 'Missing rust-toolchain.toml channel.' }
    $toolchain = $Matches[1] + '-x86_64-pc-windows-msvc'

    # Keep other projects' offline/vendor/linker configuration untouched.
    $rustupCommand = Get-Command rustup.exe -ErrorAction SilentlyContinue
    $rustup = if ($rustupCommand) { $rustupCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe' }
    $env:CARGO_HOME = Join-Path $env:LOCALAPPDATA 'vega-build\cargo'
    $env:CARGO_TARGET_DIR = Join-Path $Checkout 'target\windows-install'
    $env:RUSTC_WRAPPER = $null
    $env:RUSTC_WORKSPACE_WRAPPER = $null
    # Avoid resolving the repository pin against io's default GNU host even
    # while listing toolchains. Every Rust command below selects MSVC explicitly.
    Set-Location -LiteralPath ([IO.Path]::GetPathRoot($Checkout))
    if (-not (Test-Path -LiteralPath $rustup)) {
        $rustup = Join-Path $env:CARGO_HOME 'bin\rustup.exe'
        if (-not (Test-Path -LiteralPath $rustup)) {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            $setup = Join-Path ([IO.Path]::GetTempPath()) ('vega-rustup-' + [guid]::NewGuid() + '.exe')
            try {
                Invoke-WebRequest -UseBasicParsing -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' -OutFile $setup
                Invoke-Native $setup @('-y', '--no-modify-path', '--default-toolchain', 'none', '--default-host', 'x86_64-pc-windows-msvc')
            } finally { Remove-Item -LiteralPath $setup -ErrorAction SilentlyContinue }
        }
    }
    $installed = @(& $rustup toolchain list)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect installed Rust toolchains.' }
    if (-not ($installed | Where-Object { ($_ -split ' ')[0] -eq $toolchain })) {
        Invoke-Native $rustup @('toolchain', 'install', $toolchain, '--profile', 'minimal', '--component', 'clippy,rustfmt')
    }
    # Cargo also reads .cargo/config.toml in every cwd ancestor, independently
    # of CARGO_HOME. Build from the drive root to avoid profile-wide overrides.
    Set-Location -LiteralPath ([IO.Path]::GetPathRoot($Checkout))
    $buildArgs = @('run', $toolchain, 'cargo', 'build', '--manifest-path', (Join-Path $Checkout 'Cargo.toml'), '--release', '--locked', '--target', 'x86_64-pc-windows-msvc', '--jobs', '2')
    foreach ($package in $packages) { $buildArgs += @('-p', $package.package) }
    Invoke-Native $rustup $buildArgs
    $binaryCount = 0
    $images = @()
    foreach ($package in $packages) {
        foreach ($binary in $package.binaries) {
            $source = Join-Path $env:CARGO_TARGET_DIR "x86_64-pc-windows-msvc\release\$binary.exe"
            if (-not (Test-Path -LiteralPath $source)) { throw "Missing built executable: $source" }
            $images += [pscustomobject]@{ Source=$source; Name="$binary.exe" }
            $binaryCount++
        }
    }
    . (Join-Path $Checkout 'scripts\windows-replace.ps1')
    Install-VegaImages -Images $images -InstallDir $InstallDir
    # Until executable ports land, success means a native library build only.
    if ($binaryCount -gt 0) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $newPath = Add-PathEntry $userPath $InstallDir
        if ($newPath -cne $userPath) { [Environment]::SetEnvironmentVariable('Path', $newPath, 'User') }
    }
    if ($CoreAutostart) {
        & (Join-Path $Checkout 'scripts\windows-core-autostart.ps1') -HcPath $HcPath
    }
    Write-Output "Built $($packages.Count) Windows packages; installed $binaryCount executables in $InstallDir."
    if ($binaryCount -gt 0) { Write-Output 'Open a new shell to use the user PATH entry; running hc cores retain their environment.' }
} catch {
    Write-Error "Windows install failed: $_" -ErrorAction Continue
    exit 1
} finally {
    Set-Location $savedLocation
    foreach ($name in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process') }
    $process.PriorityClass = $savedPriority
}
