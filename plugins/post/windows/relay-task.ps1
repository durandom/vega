# Demand-only, current-user interactive task. No password, elevation or logon trigger.
param([string]$Exe, [string]$Ready, [string]$Launch, [string]$TaskName)
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject Schedule.Service
$service.Connect()
$folder = $service.GetFolder('\')
$task = $service.NewTask(0)
$task.Principal.LogonType = 3 # TASK_LOGON_INTERACTIVE_TOKEN
$task.Principal.RunLevel = 0 # TASK_RUNLEVEL_LUA
$task.Settings.ExecutionTimeLimit = 'PT0S'
$task.Settings.DisallowStartIfOnBatteries = $false
$task.Settings.StopIfGoingOnBatteries = $false
$action = $task.Actions.Create(0)
$action.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
function Literal([string]$Value) { "'" + $Value.Replace("'", "''") + "'" }
$invoke = '& ' + (Literal $Exe) + ' relay serve --ready-file ' + (Literal $Ready) + ' --launch-file ' + (Literal $Launch)
$action.Arguments = '-NoProfile -NonInteractive -WindowStyle Hidden -Command "' + $invoke + '"'
$registered = $false
try {
    $entry = $folder.RegisterTaskDefinition($TaskName, $task, 2, $null, $null, 3)
    $registered = $true
    $instance = $entry.Run($null)
    $until = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $Ready)) {
        if ([DateTime]::UtcNow -ge $until) {
            $instance.Stop()
            throw 'Worker readiness timed out; inspect native-relay/worker.log.'
        }
        Start-Sleep -Milliseconds 50
    }
} finally {
    # Deleting a definition leaves the running instance alive. No future trigger remains.
    if ($registered) { $folder.DeleteTask($TaskName, 0) }
}
