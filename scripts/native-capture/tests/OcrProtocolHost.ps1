# Test shim: execute the real JSON-lines server, replacing only native WinRT activation/read.
# No desktop, image or native evidence is created. Cold-start and error modes are deterministic.
$tokens=$null; $errors=$null
$source=Join-Path $PSScriptRoot '../Read-WindowText.ps1'
$ast=[System.Management.Automation.Language.Parser]::ParseFile($source,[ref]$tokens,[ref]$errors)
if ($errors) { throw ($errors | Out-String) }
$script:activations=0
foreach ($statement in $ast.EndBlock.Statements) {
    if ($statement -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $statement.Name -eq 'Initialize-Ocr') {
        function Initialize-Ocr {
            Start-Sleep -Milliseconds 300
            $script:activations++
            $script:engine='test engine'
        }
    } elseif ($statement -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $statement.Name -eq 'Read-Image') {
        function Read-Image([string]$Image) {
            switch ($Image) {
                'hang' { Start-Sleep -Seconds 30 }
                'error' { throw 'simulated OCR unavailable' }
                'eof' { exit 0 }
                'invalid' { $outputPipe.WriteLine('not-json'); exit 0 }
                'wrong-id' { $outputPipe.WriteLine('{"id":-1}'); exit 0 }
                'missing-result' { $outputPipe.WriteLine((@{id=$request.id} | ConvertTo-Json -Compress)); exit 0 }
            }
            @{text=$Image;words=@(@{text='fixture.org/repo';x=40;y=80;width=160;height=20});activations=$script:activations;pid=$PID}
        }
    } else { Invoke-Expression $statement.Extent.Text }
}
