# Build the pinned baseline using the existing dependency cache; never change either source tree.
param(
    [Parameter(Mandatory)][string]$ExpectedHead,
    [string]$OutDir = 'native-width-captures',
    [string]$BinaryDir = 'native-width-binaries'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$baseline = '4cc273ac30fe091fb9e898c85d004831648cb8c8'
$head = (git rev-parse HEAD)
if ($LASTEXITCODE -ne 0 -or $head -cne $ExpectedHead) { throw 'Width build checkout is not the expected PR head.' }
New-Item -ItemType Directory -Force $OutDir,$BinaryDir | Out-Null
$BinaryDir = (Resolve-Path $BinaryDir).Path
$target = Join-Path (Get-Location) 'target'
$exe = Join-Path $target 'release/project-dashboard.exe'
$worktree = Join-Path ([IO.Path]::GetTempPath()) "dashboard-width-baseline-$([Guid]::NewGuid().ToString('N'))"
$added = $false; $copied = $false
$manifest = @{head=$head;baseline=$baseline;success=$false;sources=@();startedAt=[DateTimeOffset]::UtcNow.ToString('o')}
try {
    Copy-Item $exe (Join-Path $BinaryDir 'head.exe')
    $copied = $true
    $manifest.sources += @{source='head';commit=$head;exeSha256=(Get-FileHash (Join-Path $BinaryDir 'head.exe')).Hash.ToLowerInvariant()}
    git worktree add --detach $worktree $baseline
    if ($LASTEXITCODE -ne 0) { throw 'Cannot create pinned baseline worktree.' }
    $added = $true
    if ((git -C $worktree rev-parse HEAD) -cne $baseline) { throw 'Baseline worktree revision mismatch.' }
    cargo build --release --locked -p dashboard-app --manifest-path (Join-Path $worktree 'Cargo.toml') --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'Pinned baseline release build failed.' }
    Copy-Item $exe (Join-Path $BinaryDir 'baseline.exe')
    $manifest.sources += @{source='baseline';commit=$baseline;exeSha256=(Get-FileHash (Join-Path $BinaryDir 'baseline.exe')).Hash.ToLowerInvariant()}
    $manifest.success = $true
} catch {
    $manifest.failure = $_.Exception.Message
    throw
} finally {
    $cleanupErrors = @()
    try { if ($copied) { Copy-Item (Join-Path $BinaryDir 'head.exe') $exe } } catch { $cleanupErrors += $_.Exception.Message }
    if ($added) {
        git worktree remove $worktree
        if ($LASTEXITCODE -ne 0) { $cleanupErrors += 'Could not remove owned baseline worktree.' }
    }
    if ($cleanupErrors.Count) { $manifest.success=$false; $manifest.cleanupErrors=$cleanupErrors }
    $manifest.finishedAt = [DateTimeOffset]::UtcNow.ToString('o')
    $manifest | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $OutDir 'build-manifest.json')
    if ($cleanupErrors.Count) { throw ($cleanupErrors -join '; ') }
}
