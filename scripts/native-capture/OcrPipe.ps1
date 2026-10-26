# Used by the capture and the protocol regression. One request at a time, no retries/restarts.
function Invoke-OcrRead([Diagnostics.Process]$OcrProcess,[string]$Image,[int]$Id,[int]$TimeoutMs) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $request = @{id=$Id;image=$Image} | ConvertTo-Json -Compress
    $write = $OcrProcess.StandardInput.WriteLineAsync($request)
    if (-not $write.Wait($TimeoutMs)) { throw 'OCR request write timed out.' }
    $remaining = [Math]::Max(1,$TimeoutMs-[int]$watch.ElapsedMilliseconds)
    $read = $OcrProcess.StandardOutput.ReadLineAsync()
    if (-not $read.Wait($remaining)) { throw "OCR response exceeded $TimeoutMs ms." }
    $line = $read.GetAwaiter().GetResult()
    if ($null -eq $line) { throw 'OCR host exited without a response.' }
    $response = $line | ConvertFrom-Json
    if ($response.id -ne $Id) { throw 'OCR protocol returned the wrong request identity.' }
    if ($null -ne $response.PSObject.Properties['error']) { throw "OCR host failed: $($response.error)" }
    if ($null -eq $response.PSObject.Properties['result'] -or $null -eq $response.result -or
        $null -eq $response.result.PSObject.Properties['text'] -or $null -eq $response.result.PSObject.Properties['words']) { throw 'OCR protocol returned no text/word result.' }
    $response
}
