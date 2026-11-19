<#
.SYNOPSIS
Runs cargo-mutants on dashboard-core's write, edit, board and sort code and checks the result against the mutation
policy (#101, docs/quality.md).

.DESCRIPTION
Full mode mutates every examined file and checks the score against the break threshold. Diff mode mutates only the
lines changed since -Base and requires every such mutant caught. The examined files come from the policy, so the
script and the gate can't disagree. cargo-mutants builds each mutant in its own copy of the tree, never in this
checkout. Each run starts from an empty output folder, so a report always comes from this invocation.

Exits 0 when the gate passes, 1 when it fails, 2 when the run is incomplete or an input is missing.

.EXAMPLE
pwsh scripts/Invoke-Mutants.ps1 -Mode full
pwsh scripts/Invoke-Mutants.ps1 -Mode diff -Base origin/main
#>
param(
    [ValidateSet('full', 'diff')] [string] $Mode = 'full',
    # The commit the diff starts from (diff mode).
    [string] $Base,
    [string] $Out = 'target/mutants',
    [int] $Jobs = 2
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)

$platform = if ($IsWindows) { 'windows' } elseif ($IsLinux) { 'linux' } else { 'other' }
$policy = "crates/quality/baselines/mutants-$platform.json"
if (-not (Test-Path $policy)) {
    Write-Error "No mutation policy for $platform ($policy). The gate runs on Linux, where the policy was measured." -ErrorAction Continue
    exit 2
}
$files = (Get-Content -Raw $policy | ConvertFrom-Json).files
if (-not $files) {
    Write-Error "$policy lists no files." -ErrorAction Continue
    exit 2
}
$revision = (git rev-parse HEAD).Trim()

# A fresh folder: a report left by an earlier run must never decide this one.
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null

$arguments = @('mutants', '--package', 'dashboard-core', '--jobs', $Jobs, '--output', $Out, '--no-shuffle')
foreach ($file in $files) { $arguments += @('--file', $file) }
if ($Mode -eq 'diff') {
    if (-not $Base) {
        Write-Error '-Base is required in diff mode.' -ErrorAction Continue
        exit 2
    }
    $diff = Join-Path $Out 'changes.diff'
    git diff --no-color --no-ext-diff $Base $revision -- @files | Set-Content -Path $diff -Encoding utf8NoBOM
    if ($LASTEXITCODE) { exit 2 }
    if ((Get-Item $diff).Length -eq 0) {
        $note = "No examined file changed since $Base; nothing to mutate."
        Set-Content -Path (Join-Path $Out "mutants-diff-$platform.md") -Value "# dashboard-core mutation report (diff, $platform)`n`n- Revision: ``$revision```n- $note`n- Gate: pass"
        Write-Output $note
        exit 0
    }
    $arguments += @('--in-diff', $diff)
}

Write-Output "cargo $($arguments -join ' ')"
& cargo @arguments
$mutantsExit = $LASTEXITCODE
# cargo-mutants' own results are 0 (all caught), 2 (missed), 3 (timeout) and 4 (the unmutated baseline failed); the gate
# reads those from the outcomes. Anything else (1 usage, 70 internal error) means the run didn't finish.
if ($mutantsExit -notin 0, 2, 3, 4) {
    Write-Error "cargo-mutants exited ${mutantsExit}: the run is incomplete." -ErrorAction Continue
    exit 2
}
$outcomes = Join-Path $Out 'mutants.out/outcomes.json'
if (-not (Test-Path $outcomes)) {
    # cargo-mutants writes no outcomes when a non-empty diff holds no mutable code (comments only, say).
    if ($Mode -eq 'diff' -and $mutantsExit -eq 0) {
        Write-Output 'The changed lines hold no mutants.'
        exit 0
    }
    Write-Error "cargo-mutants exited $mutantsExit without ${outcomes}: the run is incomplete." -ErrorAction Continue
    exit 2
}

# The gate decides from the outcomes, not from cargo-mutants' exit code, so missed, timeout and baseline failures all
# reach it.
cargo run --quiet --locked -p quality -- mutants --src crates/dashboard-core/src --outcomes $outcomes --mode $Mode `
    --revision $revision --policy $policy --out $Out
exit $LASTEXITCODE
