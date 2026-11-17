# Image replacement for the source installer; dot-source for fixture tests.
function Install-VegaImages {
    param([array]$Images, [string]$InstallDir)
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $id = [guid]::NewGuid().ToString('N')
    $staged = @()
    $replaced = @()
    $relay = $Images | Where-Object { $_.Name -eq 'vega-post.exe' } | Select-Object -First 1
    $restart = $false
    if ($relay) {
        $text = & $relay.Source relay status --json
        if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect the native worker before updating.' }
        $status = $text | ConvertFrom-Json
        $relayPath = Join-Path $InstallDir 'vega-post.exe'
        if ($status.running -and -not $status.exe) {
            throw 'Old foreground worker has no stop protocol; stop its kept terminal with Ctrl-C once, then rerun.'
        }
        $restart = $status.running -and ($status.exe -ieq $relayPath)
    }
    try {
        # Stage everything before renaming. Windows permits renaming mapped
        # images, but does not permit overwriting their bytes.
        foreach ($image in $Images) {
            $destination = Join-Path $InstallDir $image.Name
            $entry = [pscustomobject]@{ Destination=$destination; Stage="$destination.new-$id"; Old="$destination.old-$id"; HadOld=(Test-Path -LiteralPath $destination) }
            $staged += $entry
            Copy-Item -LiteralPath $image.Source -Destination $entry.Stage
        }
        foreach ($entry in $staged) {
            if ($entry.HadOld) { Move-Item -LiteralPath $entry.Destination -Destination $entry.Old }
            $replaced += $entry
            Move-Item -LiteralPath $entry.Stage -Destination $entry.Destination
        }
        if ($restart) {
            & $relayPath relay stop
            if ($LASTEXITCODE -ne 0) { throw 'Could not stop the previous native worker.' }
            & $relayPath relay start
            if ($LASTEXITCODE -ne 0) { throw 'Could not restart the native worker.' }
        }
    } catch {
        # Keep recoverable old images on failure. A worker may still map either
        # generation; do not remove its installed executable.
        foreach ($entry in $replaced) {
            Write-Warning "Update recovery: $($entry.Old) -> $($entry.Destination)"
        }
        throw
    } finally {
        foreach ($entry in $staged) {
            Remove-Item -LiteralPath $entry.Stage -ErrorAction SilentlyContinue
        }
    }
    # Other binaries can still be mapped. Retry their cleanup next install.
    Get-ChildItem -LiteralPath $InstallDir -Filter '*.exe.old-*' | ForEach-Object {
        Remove-Item -LiteralPath $_.FullName -ErrorAction SilentlyContinue
    }
}
