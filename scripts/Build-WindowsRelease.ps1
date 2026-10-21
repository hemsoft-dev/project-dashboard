<#
.SYNOPSIS
Builds project-dashboard.exe in release, checks it, and zips it for a GitHub release (#3).

.DESCRIPTION
Checks that the executable is a GUI-subsystem program (no console window), embeds icon resource 1 and the version
block, and starts and opens its "Project Dashboard" window. Writes dist/project-dashboard-<version>-windows-x64.zip,
its .sha256, version.txt, and window.png (a screenshot of the started app) to OutDir.

.PARAMETER Tag
The release tag (v<version>). When given, it must match the workspace version.
#>
[CmdletBinding()]
param(
    [string]$OutDir = 'dist',
    [string]$Tag = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
        Where-Object name -eq 'dashboard-app' | Select-Object -ExpandProperty version
    if ($Tag -and $Tag -ne "v$version") {
        throw "The tag $Tag doesn't match the workspace version $version; set [workspace.package] version first."
    }
    Write-Information "Building Project Dashboard $version" -InformationAction Continue

    cargo build --release --locked -p dashboard-app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    $exe = Join-Path $root 'target/release/project-dashboard.exe'

    # PE header: e_lfanew at 0x3C; the optional header's Subsystem field sits 68 bytes into it (2 = Windows GUI).
    $bytes = [System.IO.File]::ReadAllBytes($exe)
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    $subsystem = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)
    if ($subsystem -ne 2) { throw "project-dashboard.exe has subsystem $subsystem; expected 2 (Windows GUI, no console)" }

    Add-Type -Namespace Win32 -Name Resources -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern System.IntPtr LoadLibraryEx(string path, System.IntPtr file, uint flags);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern System.IntPtr FindResource(System.IntPtr module, System.IntPtr name, System.IntPtr type);
[DllImport("kernel32.dll")]
public static extern bool FreeLibrary(System.IntPtr module);
'@
    # LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE; RT_GROUP_ICON is 14.
    $module = [Win32.Resources]::LoadLibraryEx($exe, [IntPtr]::Zero, 0x22)
    try {
        if ([Win32.Resources]::FindResource($module, [IntPtr]1, [IntPtr]14) -eq [IntPtr]::Zero) {
            throw 'project-dashboard.exe has no icon resource 1'
        }
    } finally {
        [void][Win32.Resources]::FreeLibrary($module)
    }

    $info = (Get-Item $exe).VersionInfo
    foreach ($check in @(
            @('ProductName', 'Project Dashboard'),
            @('FileDescription', 'Project Dashboard'),
            @('CompanyName', 'HemSoft Developments'),
            @('ProductVersion', $version),
            @('OriginalFilename', 'project-dashboard.exe'))) {
        if ($info.($check[0]) -ne $check[1]) { throw "$($check[0]) is '$($info.($check[0]))'; expected '$($check[1])'" }
    }

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $OutDir = (Resolve-Path $OutDir).Path

    # Start it with an empty config folder, wait for its window, and photograph the screen.
    $config = Join-Path ([IO.Path]::GetTempPath()) "project-dashboard-release-$PID"
    New-Item -ItemType Directory -Force -Path $config | Out-Null
    Set-Content -Path (Join-Path $config 'config.toml') -Value 'repositories = []'
    $env:PROJECT_DASHBOARD_CONFIG_DIR = $config
    $process = Start-Process -FilePath $exe -PassThru
    try {
        $deadline = (Get-Date).AddSeconds(60)
        do {
            Start-Sleep -Milliseconds 500
            $process.Refresh()
            if ($process.HasExited) { throw "project-dashboard.exe exited with code $($process.ExitCode) before showing its window" }
        } until ($process.MainWindowTitle -eq 'Project Dashboard' -or (Get-Date) -gt $deadline)
        if ($process.MainWindowTitle -ne 'Project Dashboard') {
            throw "No 'Project Dashboard' window within 60 s (main window title: '$($process.MainWindowTitle)')"
        }
        Start-Sleep -Seconds 3
        Add-Type -AssemblyName System.Windows.Forms, System.Drawing
        $bounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $bitmap = New-Object System.Drawing.Bitmap $bounds.Width, $bounds.Height
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
        $bitmap.Save((Join-Path $OutDir 'window.png'), [System.Drawing.Imaging.ImageFormat]::Png)
        $graphics.Dispose(); $bitmap.Dispose()
        Write-Information "Started: window '$($process.MainWindowTitle)', PID $($process.Id)" -InformationAction Continue
    } finally {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
        Remove-Item $env:PROJECT_DASHBOARD_CONFIG_DIR -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item Env:PROJECT_DASHBOARD_CONFIG_DIR
    }

    $name = "project-dashboard-$version-windows-x64"
    $stage = Join-Path ([IO.Path]::GetTempPath()) $name
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $stage | Out-Null
    Copy-Item $exe, (Join-Path $root 'README.md'), (Join-Path $root 'LICENSE') $stage
    $zip = Join-Path $OutDir "$name.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -Path "$zip.sha256" -Value "$hash  $name.zip" -NoNewline
    Set-Content -Path (Join-Path $OutDir 'version.txt') -Value $version -NoNewline
    Write-Information "Packaged $zip ($hash)" -InformationAction Continue
} finally {
    Pop-Location
}
