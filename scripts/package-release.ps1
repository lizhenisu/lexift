param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

function Update-ShellItem {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ($env:OS -ne "Windows_NT") { return }
    if (-not ("LexiftShellChangeNotifier" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class LexiftShellChangeNotifier
{
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    public static extern void SHChangeNotify(
        uint eventId,
        uint flags,
        string item1,
        IntPtr item2);
}
"@
    }

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    # SHCNE_UPDATEITEM with SHCNF_PATHW | SHCNF_FLUSH refreshes only this path.
    [LexiftShellChangeNotifier]::SHChangeNotify(0x00002000, 0x00001005, $resolved, [IntPtr]::Zero)
}

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = ((Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"$').Matches.Groups[1].Value)
if (-not $version) { throw "Could not read workspace version" }

$dist = Join-Path $root "dist"
$installerDir = Join-Path $dist "installer"
$portableDir = Join-Path $dist "portable\Lexift-$version-windows-x86_64"
Remove-Item -LiteralPath $installerDir -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Split-Path -Parent $portableDir) -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $installerDir, $portableDir | Out-Null

if (-not $SkipBuild) {
    cargo build --release --bin lexift
    if ($LASTEXITCODE -ne 0) { throw "Release build failed" }
}

$exe = Join-Path $root "target\release\lexift.exe"
& "$PSScriptRoot\sign-windows.ps1" -Path $exe

cargo packager --release
if ($LASTEXITCODE -ne 0) { throw "cargo-packager failed" }

$generatedInstaller = Get-ChildItem -LiteralPath $installerDir -Filter *.exe | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $generatedInstaller) { throw "NSIS installer was not produced" }
$installer = Join-Path $dist "Lexift-$version-windows-x86_64-setup.exe"
Move-Item -LiteralPath $generatedInstaller.FullName -Destination $installer -Force
& "$PSScriptRoot\sign-windows.ps1" -Path $installer
Update-ShellItem -Path $installer
Update-ShellItem -Path $exe

Copy-Item -LiteralPath $exe -Destination (Join-Path $portableDir "lexift.exe")
Copy-Item -LiteralPath LICENSE -Destination $portableDir
Copy-Item -LiteralPath README.md -Destination $portableDir
$portable = Join-Path $dist "Lexift-$version-windows-x86_64-portable.zip"
Compress-Archive -Path "$portableDir\*" -DestinationPath $portable -CompressionLevel Optimal -Force

$checksum = Join-Path $dist "SHA256SUMS.txt"
$entries = @($installer, $portable) | ForEach-Object {
    $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $_
    "$($hash.Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($_))"
}
Set-Content -LiteralPath $checksum -Value $entries -Encoding ascii

Write-Host "Release artifacts:"
Get-Item $installer, $portable, $checksum | Select-Object Name, Length, LastWriteTime
