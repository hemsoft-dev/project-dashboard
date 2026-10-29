# Run over SSH, including session 0. The app is launched directly on the user's
# interactive desktop; no PowerShell console is started there. UI state stays in AppData.
param(
    [Parameter(Mandatory)][string]$Package,
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ProjectDashboard",
    [string]$InteractiveUser = $env:USERNAME
)
$ErrorActionPreference = 'Stop'
$stage = Join-Path $env:TEMP ('dashboard-install-' + [Guid]::NewGuid().ToString('N'))
$task = 'DashboardRelaunch-' + [Guid]::NewGuid().ToString('N')
try {
    Expand-Archive -LiteralPath $Package -DestinationPath $stage
    if (!(Test-Path "$stage\project-dashboard.exe")) { throw 'Package has no dashboard executable' }
    # Move/resize saves are debounced. Allow the last event to settle before stopping
    # a process in another session, where CloseMainWindow cannot reach its window.
    Start-Sleep -Milliseconds 600
    Get-Process project-dashboard -ErrorAction SilentlyContinue | Where-Object Path -eq "$InstallDir\project-dashboard.exe" | Stop-Process -Force
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    Copy-Item "$stage\*" $InstallDir -Recurse -Force
    $action = New-ScheduledTaskAction -Execute "$InstallDir\project-dashboard.exe" -WorkingDirectory $InstallDir
    $principal = New-ScheduledTaskPrincipal -UserId $InteractiveUser -LogonType Interactive
    Register-ScheduledTask -TaskName $task -Action $action -Principal $principal | Out-Null
    Start-ScheduledTask -TaskName $task
    Start-Sleep -Seconds 3
    $running = Get-Process project-dashboard -ErrorAction SilentlyContinue | Where-Object Path -eq "$InstallDir\project-dashboard.exe"
    if (!$running -or !($running | Where-Object SessionId -ne 0)) { throw 'Dashboard did not launch in an interactive session' }
    $running | Select-Object Id, SessionId, Path, FileVersion
} finally {
    if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) { Unregister-ScheduledTask -TaskName $task -Confirm:$false }
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
}
