# Constant synthetic data for both executables and every size/profile. No state transitions.
param([Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$template = Get-Content (Join-Path $PSScriptRoot '../../crates/dashboard-core/tests/fixtures/status-clean.json') -Raw
$stamp = [DateTimeOffset]::UtcNow.AddMinutes(-14).ToString('o')
$responses = @{}
$projects = @()
foreach ($kind in @('long','short')) {
    $repo = "fixture/$kind"
    $projects += @{slug=$kind;repo=$repo;priority=1;tracked=$true}
    $status = $template | ConvertFrom-Json -AsHashtable
    [void]$status.Remove('health'); [void]$status.Remove('verdict')
    $status.repository = @{nameWithOwner=$repo;defaultBranch='main'}
    $status.generatedAt = $stamp
    $status.pullRequests.open = @(@{number=27;title='SYNTHETIC older PR';url="https://github.com/$repo/pull/27";updatedAt='2020-01-01T00:00:00Z'})
    $status.pullRequests.merged = @()
    $status.issues = @(@{number=27;title='SYNTHETIC older issue';state='OPEN';url="https://github.com/$repo/issues/27";updatedAt='2020-01-01T00:00:00Z'})
    $run = $status.workflowRuns[0]
    $run.workflow = if ($kind -eq 'long') { 'SYNTHETIC LONG Build and test all Windows release packages and validate embedded executable resources across all supported architectures and deployment environments' } else { 'CI' }
    $run.title = if ($kind -eq 'long') { 'DESCRIPTION uses residual width for synthetic dashboard activity and retains every row control' } else { 'SYNTHETIC SHORT description uses remaining activity width' }
    $run.url = "https://github.com/$repo/actions/runs/27"
    $run.createdAt=$stamp; $run.startedAt=$stamp; $run.updatedAt=$stamp
    $responses["/v1/projects/$kind/status"] = @{project=$kind;generatedAt=$stamp;lastError=$null;status=$status}
    $responses["/v1/board?project=$kind"] = @{generatedAt=$stamp;columns=@(
        @{status='ready';title='Ready';items=@();truncated=$false},
        @{status='in_progress';title='In progress';items=@();truncated=$false},
        @{status='waiting_on_franz';title='Waiting on Franz';items=@();truncated=$false},
        @{status='blocked';title='Blocked';items=@();truncated=$false})}
    $responses["/v1/work?project=$kind&limit=500"] = @{items=@();nextCursor=$null}
}
$responses['/v1/projects'] = @{projects=$projects}
# Uploaded response payloads establish identical input data for baseline/head.
$responses | ConvertTo-Json -Depth 30 | Set-Content (Join-Path $Root 'fixture-data.json')
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
$listener.Start()
try {
    [IO.File]::WriteAllText((Join-Path $Root 'port.tmp'),[string]$listener.LocalEndpoint.Port)
    [IO.File]::Move((Join-Path $Root 'port.tmp'),(Join-Path $Root 'port.txt'))
    while ($true) {
        $client = $listener.AcceptTcpClient()
        try {
            $client.ReceiveTimeout=2000; $client.SendTimeout=2000
            $stream=$client.GetStream()
            $reader=[IO.StreamReader]::new($stream,[Text.Encoding]::ASCII,$false,1024,$true)
            $line=$reader.ReadLine(); $auth=''
            while ($header=$reader.ReadLine()) { if ($header -match '^Authorization:\s*(.+)$') { $auth=$Matches[1] } }
            $path=$line.Split(' ')[1]
            $code=200
            if ($auth -ne 'Bearer synthetic-issue27-token') { $code=401; $body=@{error=@{code='unauthorized';message='synthetic fixture auth failure'}} }
            elseif ($responses.ContainsKey($path)) { $body=$responses[$path] }
            else { $code=404; $body=@{error=@{code='not_found';message='synthetic fixture unknown route'}} }
            $bytes=[Text.Encoding]::UTF8.GetBytes(($body | ConvertTo-Json -Depth 30 -Compress))
            $headers=[Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $code Fixture`r`nContent-Type: application/json`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n")
            $stream.Write($headers); $stream.Write($bytes); $stream.Flush()
            $session = (Get-Content (Join-Path $Root 'session.txt') -Raw).Trim()
            @{session=$session;path=$path;status=$code} | ConvertTo-Json -Compress | Add-Content (Join-Path $Root 'requests.jsonl')
        } finally { $client.Dispose() }
    }
} finally { $listener.Stop() }
