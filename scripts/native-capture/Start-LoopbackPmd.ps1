param([Parameter(Mandatory)][string]$Root)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Read-only synthetic pmd v0.1 contract. The controller changes only state.txt.
$base = Get-Content (Join-Path $PSScriptRoot '../../crates/dashboard-core/tests/fixtures/status-clean.json') -Raw | ConvertFrom-Json -AsHashtable
[void]$base.Remove('health'); [void]$base.Remove('verdict')
$base.repository = @{nameWithOwner='fixture-org/synthetic-dashboard';defaultBranch='main'}
$base.pullRequests = @{open=@();merged=@();mergedLimit=0}
$base.workflowRuns = @(); $base.workflowRunsPerfect = $true
$generated = [DateTimeOffset]::UtcNow.AddMinutes(-10).ToString('o')
$base.generatedAt = $generated
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,0)
$listener.Start()
try {
    [IO.File]::WriteAllText((Join-Path $Root 'port.tmp'),[string]$listener.LocalEndpoint.Port)
    [IO.File]::Move((Join-Path $Root 'port.tmp'),(Join-Path $Root 'port.txt'))
    while ($true) {
        $client=$listener.AcceptTcpClient()
        try {
            $client.ReceiveTimeout=2000; $client.SendTimeout=2000
            $stream=$client.GetStream(); $reader=[IO.StreamReader]::new($stream,[Text.Encoding]::ASCII,$false,1024,$true)
            $line=$reader.ReadLine(); $auth=''
            while ($header=$reader.ReadLine()) { if ($header -match '^Authorization:\s*(.+)$') { $auth=$Matches[1] } }
            $target=$line.Split(' ')[1]; $uri=[Uri]('http://127.0.0.1'+$target)
            $mode=(Get-Content (Join-Path $Root 'state.txt') -Raw).Trim()
            $code=200; $body=$null
            if ($auth -ne 'Bearer synthetic-issue31-token') { $code=401; $body=@{error=@{code='unauthorized';message='synthetic fixture auth failure'}} }
            else {
                $label='SYNTHETIC FIXTURE '+$mode.ToUpperInvariant()
                $item=@{id='synthetic-issue31-item';project='synthetic-dashboard';kind='issue';number=31;title=$label;url='https://github.com/fixture-org/synthetic-dashboard/issues/31';status='ready'}
                switch ($uri.AbsolutePath) {
                    '/v1/projects' { $body=@{projects=@(@{slug='synthetic-dashboard';repo='fixture-org/synthetic-dashboard';priority=1;tracked=$true})} }
                    '/v1/projects/synthetic-dashboard/status' {
                        if ($mode -eq 'unavailable') { $code=404; $body=@{error=@{code='no_status';message='SYNTHETIC FIXTURE no_status'}} }
                        else {
                            $base.issues=@(@{number=31;title=$label;state='OPEN';updatedAt=[DateTimeOffset]::UtcNow.ToString('o');url=$item.url})
                            $body=@{project='synthetic-dashboard';generatedAt=$generated;status=$base;lastError=$null}
                            if ($mode -eq 'stale') { $base.issues[0].title='SYNTHETIC FIXTURE NORMAL'; $body.lastError=@{message='SYNTHETIC FIXTURE STALE upstream refresh failed';at=[DateTimeOffset]::UtcNow.AddMinutes(-1).ToString('o')} }
                        }
                    }
                    '/v1/board' { $body=@{generatedAt=$generated;columns=@(
                        @{status='ready';title='Ready';items=@($item);truncated=$true},
                        @{status='in_progress';title='In progress';items=@();truncated=$false},
                        @{status='waiting_on_franz';title='Waiting on Franz';items=@();truncated=$false},
                        @{status='blocked';title='Blocked';items=@();truncated=$false})} }
                    '/v1/work' {
                        if ($uri.Query -match 'cursor=') { $body=@{items=@();nextCursor=$null} }
                        else { $body=@{items=@($item);nextCursor='synthetic-next-page'} }
                    }
                    default { $code=404; $body=@{error=@{code='not_found';message='synthetic fixture unknown route'}} }
                }
            }
            $bytes=[Text.Encoding]::UTF8.GetBytes(($body | ConvertTo-Json -Depth 30 -Compress))
            $headers=[Text.Encoding]::ASCII.GetBytes("HTTP/1.1 $code Fixture`r`nContent-Type: application/json`r`nContent-Length: $($bytes.Length)`r`nConnection: close`r`n`r`n")
            $stream.Write($headers); $stream.Write($bytes); $stream.Flush()
            @{state=$mode;path=$uri.AbsolutePath;query=$uri.Query;status=$code;at=[DateTimeOffset]::UtcNow.ToString('o')} | ConvertTo-Json -Compress | Add-Content (Join-Path $Root 'requests.jsonl')
            if (($code -eq 200 -and $uri.AbsolutePath -eq '/v1/work' -and $uri.Query -match 'cursor=') -or
                ($mode -eq 'unavailable' -and $code -eq 404 -and $uri.AbsolutePath.EndsWith('/status'))) {
                [IO.File]::WriteAllText((Join-Path $Root "completed-$mode"),'complete')
            }
        } finally { $client.Dispose() }
    }
} finally { $listener.Stop() }
