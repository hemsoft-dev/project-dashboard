# Replay the actual assertion code and call-site patterns. No window, image or OCR process is created.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$tokens=$null; $errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'Invoke-Capture.ps1'),[ref]$tokens,[ref]$errors)
if ($errors) { throw ($errors | Out-String) }
$capture=$ast.Find({param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Capture-State'},$true)
$loop=@($capture.Body.EndBlock.Statements | Where-Object { $_ -is [System.Management.Automation.Language.ForEachStatementAst] })
if ($loop.Count -ne 1) { throw 'Cannot locate the actual capture assertion loop.' }
$script:validate=[scriptblock]::Create($loop[0].Extent.Text)
$current=(Get-Content (Join-Path $PSScriptRoot 'tests/ocr-stale-40245709913.json') -Raw | ConvertFrom-Json).text
$old=(Get-Content (Join-Path $PSScriptRoot 'tests/ocr-stale-40244579026.json') -Raw | ConvertFrom-Json).text
$script:replay=$current
$script:statePatterns=@{}
function Capture-State([string]$State,[string[]]$Patterns) {
    $script:statePatterns[$State]=$Patterns
    $text=@{text=$script:replay}
    $visible=[regex]::Replace($text.text,'\s+',' ')
    & $script:validate
}
$nodes=@($ast.FindAll({param($node)
    ($node -is [System.Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$normalRows') -or
    ($node -is [System.Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Capture-State')
},$true) | Sort-Object { $_.Extent.StartOffset })
if ($nodes.Count -ne 6) { throw 'Expected the retained-row patterns and all five capture calls.' }
Invoke-Expression $nodes[0].Extent.Text
$calls=@{}
foreach ($node in $nodes[1..5]) {
    $state=$node.CommandElements[1].Value
    $calls[$state]=$node.Extent.Text
}
# Both runs log stale OCR only. The corrected snapshot contains the retained NORMAL rows,
# so every normal/stale pattern must match it despite the unread small Cached pill.
foreach ($state in @('normal','stale')) { Invoke-Expression $calls[$state] }
Write-Output 'Actual logged OCR: every normal/stale pattern passed on run 40245709913.'
$script:replay=$old
Capture-State 'old-stale-common-and-error' @($script:statePatterns.stale | Where-Object { $_ -notmatch 'SYNTHETIC FIXTURE NORMAL' })
Write-Output 'Common/error patterns also passed on run 40244579026; its incorrect STALE rows are tested as a failure below.'
function Reject([string]$State,[string]$Value,[string]$Reason) {
    $script:replay=$Value; $rejected=$false
    try { Invoke-Expression $calls[$State] } catch { $rejected=$true }
    if (-not $rejected) { throw "Capture incorrectly accepted $Reason" }
}
Reject 'stale' $old 'the changed STALE Board/work rows from run 40244579026'
Reject 'stale' ($current.Replace('pmd last refresh failed','no error')) 'missing refresh error'
Reject 'stale' ($current.Replace('pmd status generated 660s ago','unknown age')) 'missing data age'
Reject 'stale' ($current.Replace('SYNTHETIC FIXTURE NORMAL','SYNTHETIC FIXTURE WRONG')) 'changed last-good rows'
Reject 'stale' ($current.Replace('truncated, complete list below','')) 'missing truncation notice'
Reject 'stale' ($current.Replace('Unavailable: local health is not supplied by pmd','')) 'missing unavailable local health'
# These states have not been captured yet. Project their large labels/error onto the logged
# layout to check the selectors, without claiming new native evidence.
$script:replay=$current.Replace('SYNTHETIC FIXTURE STALE upstream refresh failed','pmd has no_status for this project yet.')
Invoke-Expression $calls.unavailable
Reject 'unavailable' $current 'a missing no_status error'
foreach ($pair in @(@('refresh','REFRESHED'),@('board','BOARD'))) {
    $script:replay=$current.Replace('SYNTHETIC FIXTURE NORMAL',"SYNTHETIC FIXTURE $($pair[1])")
    Invoke-Expression $calls[$pair[0]]
    if ($pair[0] -eq 'board') { Reject 'board' ($script:replay.Replace('Blocked','')) 'a missing Board column' }
    Reject $pair[0] $current 'the old NORMAL data after a successful state switch'
}
Write-Output 'Earlier fixture corruption and missing error/age/data are rejected; uncaptured-state selectors passed text-only checks.'
