# Run over SSH. Launches the GUI exe directly, never a console on the desktop.
param([Parameter(Mandatory)][string]$Exe, [string]$OutDir="$env:TEMP\dashboard-53-test", [switch]$MigrateLegacy)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
if (!(Test-Path -LiteralPath $Exe -PathType Leaf)) { throw 'Dashboard executable is missing' }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$statePath="$env:APPDATA\ProjectDashboard\ui-state.json"
$original=Get-Content $statePath -Raw | ConvertFrom-Json
Copy-Item $statePath "$OutDir\ui-state-before.json" -Force
$principal=New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive
function Run-Once($Action) {
 $task='Dashboard53-'+[Guid]::NewGuid().ToString('N')
 try {
  Register-ScheduledTask -TaskName $task -Action $Action -Principal $principal | Out-Null
  Start-ScheduledTask $task
  Start-Sleep -Seconds 4
 } finally { Unregister-ScheduledTask $task -Confirm:$false }
}
function Read-Placement([string]$Name,[string]$Mode='Read') {
 $output=Join-Path $OutDir ($Name+'.json')
 Remove-Item -LiteralPath $output,($output+'.png') -Force -ErrorAction SilentlyContinue
 $reader=Join-Path $PSScriptRoot 'Read-Placement.ps1'
 Run-Once (New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -NonInteractive -WindowStyle Hidden -File "'+$reader+'" -Output "'+$output+'" -Mode '+$Mode))
 if (!(Test-Path $output)) { throw "Native read failed: $Name" }
 Get-Content $output -Raw | ConvertFrom-Json
}
function Rect($r) { "$($r.L),$($r.T),$($r.Ri),$($r.B)" }
$before=Read-Placement 'before'
if ($MigrateLegacy) {
 # Upgrade from the live native placement, which is more accurate than the
 # legacy logical client bounds. Other preferences are preserved verbatim.
 $r=$before.placement.Rect
 $native=@{rect=@($r.L,$r.T,$r.Ri,$r.B);monitor=$before.monitor;dpi=$before.dpi;maximized=($before.placement.Show -eq 3 -or ($before.placement.Show -eq 2 -and ($before.placement.Flags -band 2)))}
 $original | Add-Member -NotePropertyName nativePlacement -NotePropertyValue $native -Force
 $original | ConvertTo-Json -Depth 10 | Set-Content $statePath
}
for($i=1;$i -le 2;$i++) {
 Get-Process project-dashboard | Stop-Process -Force
 Run-Once (New-ScheduledTaskAction -Execute $Exe -WorkingDirectory (Split-Path $Exe))
 $after=Read-Placement "restart-$i"
 if ((Rect $after.placement.Rect) -ne (Rect $before.placement.Rect) -or $after.placement.Show -ne $before.placement.Show -or (Rect $after.visible) -ne (Rect $before.visible)) { throw "Placement changed on restart $i" }
}
try {
 $normal=Read-Placement 'unmaximized' 'Normal'
 if ((Rect $normal.placement.Rect) -ne (Rect $before.placement.Rect)) { throw 'Unmaximize changed normal bounds' }
} finally { $restored=Read-Placement 'restored' $(if($before.placement.Show -eq 3){'Maximized'}else{'Normal'}) }
$final=Get-Content $statePath -Raw | ConvertFrom-Json
if($final.zoom -ne $original.zoom -or ($final.columns | ConvertTo-Json -Compress) -ne ($original.columns | ConvertTo-Json -Compress)) {throw 'Zoom or widths changed'}
# Simulate a disconnected monitor without changing the user's desktop topology.
$savedState=Get-Content $statePath -Raw
try {
 Get-Process project-dashboard | Stop-Process -Force
 $missing=$savedState | ConvertFrom-Json
 $missing.nativePlacement.monitor='disconnected-fixture'
 $missing.nativePlacement.rect=@(30000,30000,31953,31344)
 $missing | ConvertTo-Json -Depth 10 | Set-Content $statePath
 Run-Once (New-ScheduledTaskAction -Execute $Exe -WorkingDirectory (Split-Path $Exe))
 $fallback=Read-Placement 'missing-monitor'
 if ($fallback.placement.Rect.L -ge 30000 -or $fallback.placement.Rect.T -ge 30000) { throw 'Missing-monitor fallback stayed offscreen' }
} finally {
 Get-Process project-dashboard | Stop-Process -Force
 Set-Content $statePath $savedState
 Run-Once (New-ScheduledTaskAction -Execute $Exe -WorkingDirectory (Split-Path $Exe))
 $recovered=Read-Placement 'recovered'
}
@{before=$before;after=$after;normal=$normal;restored=$restored;zoom=$final.zoom;success=$true} | ConvertTo-Json -Depth 10 | Set-Content "$OutDir\result.json"
Get-Content "$OutDir\result.json"
