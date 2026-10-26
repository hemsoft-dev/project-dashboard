# One Windows PowerShell 5.1 host retains its WinRT OCR engine for the whole attempt.
# Protocol: one {id,image} JSON request per stdin line, one JSON response per stdout line.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
# Read/write the redirected pipes directly. Console encoding setters in .NET Framework
# call SetConsoleCP and can fail in a CreateNoWindow process; no console/settings are needed.
$inputPipe = [IO.StreamReader]::new([Console]::OpenStandardInput(),[Text.UTF8Encoding]::new($false))
$outputPipe = [IO.StreamWriter]::new([Console]::OpenStandardOutput(),[Text.UTF8Encoding]::new($false))
$outputPipe.AutoFlush = $true
$script:engine = $null
function Initialize-Ocr {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $null = [Windows.Storage.StorageFile, Windows.Storage, ContentType=WindowsRuntime]
    $null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType=WindowsRuntime]
    $null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
    $null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType=WindowsRuntime]
    $null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType=WindowsRuntime]
    $null = [Windows.Media.Ocr.OcrResult, Windows.Foundation, ContentType=WindowsRuntime]
    $script:asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
        $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetParameters().Count -eq 1 -and
        $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
    } | Select-Object -First 1
    $script:engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
    if ($null -eq $script:engine) { throw 'No installed OCR recognition language.' }
}
function Await($Operation, [Type]$Type) {
    $task = $script:asTask.MakeGenericMethod($Type).Invoke($null, @($Operation))
    # The controller bounds the entire request, including activation and all async operations.
    # A stuck operation terminates this owned host; it cannot outlive the capture attempt.
    $task.Wait()
    $task.Result
}
function Read-Image([string]$Image) {
    $file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($Image)) ([Windows.Storage.StorageFile])
    $stream = Await ($file.OpenAsync(0)) ([Windows.Storage.Streams.IRandomAccessStream])
    try {
        $decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
        $bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
        try {
            $result = Await ($script:engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])
            $words = @($result.Lines | ForEach-Object { $_.Words } | ForEach-Object {
                @{text=$_.Text;x=$_.BoundingRect.X;y=$_.BoundingRect.Y;width=$_.BoundingRect.Width;height=$_.BoundingRect.Height}
            })
            @{text=$result.Text;words=$words}
        } finally { $bitmap.Dispose() }
    } finally { $stream.Dispose() }
}
while ($null -ne ($line = $inputPipe.ReadLine())) {
    $request = $null
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $request = $line | ConvertFrom-Json
        if ($null -eq $script:engine) { Initialize-Ocr }
        $result = Read-Image $request.image
        $response = @{id=$request.id;result=$result;durationMs=$watch.ElapsedMilliseconds}
    } catch {
        $response = @{id=$(if ($null -ne $request) { $request.id } else { $null });error=$_.Exception.Message;durationMs=$watch.ElapsedMilliseconds}
    }
    $outputPipe.WriteLine(($response | ConvertTo-Json -Depth 8 -Compress))
}
$inputPipe.Dispose(); $outputPipe.Dispose()
