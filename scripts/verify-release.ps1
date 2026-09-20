param(
    [string]$Executable = "target/release/lexift.exe",
    [string]$RenderedInstallerScript = "dist/installer/.cargo-packager/nsis/x64/installer.nsi"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $Executable)) { throw "Missing executable: $Executable" }

$item = Get-Item -LiteralPath $Executable
$version = $item.VersionInfo
if ($version.ProductName -ne "Lexift") { throw "Unexpected ProductName: $($version.ProductName)" }
if ($version.ProductVersion -notlike "0.1.0*") { throw "Unexpected ProductVersion: $($version.ProductVersion)" }
if ($version.FileDescription -ne "Lexift — Translate Everywhere") { throw "Unexpected FileDescription: $($version.FileDescription)" }
if ($version.OriginalFilename -ne "lexift.exe") { throw "Unexpected OriginalFilename: $($version.OriginalFilename)" }

$bytes = [IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Executable))
$peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
$optionalHeader = $peOffset + 24
$subsystem = [BitConverter]::ToUInt16($bytes, $optionalHeader + 68)
if ($subsystem -ne 2) { throw "Executable is not a Windows GUI subsystem binary (value: $subsystem)" }

if (-not (Test-Path -LiteralPath $RenderedInstallerScript)) {
    throw "Missing rendered NSIS script: $RenderedInstallerScript"
}

$installerScript = Get-Content -LiteralPath $RenderedInstallerScript -Raw
$requiredInstallerFragments = @(
    'WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$INSTDIR"',
    'RmDir /r "$APPDATA\Lexift"',
    'IfFileExists "$APPDATA\Lexift"',
    'RmDir /r "$LOCALAPPDATA\Lexift"',
    'IfFileExists "$LOCALAPPDATA\Lexift"',
    'CredDeleteW',
    '"/DELETEUSERDATA"',
    'Function un.onUninstSuccess',
    'SetErrorLevel 1'
)
foreach ($fragment in $requiredInstallerFragments) {
    if (-not $installerScript.Contains($fragment)) {
        throw "Rendered NSIS script is missing required cleanup logic: $fragment"
    }
}

foreach ($forbidden in @('$APPDATA/Lexift', '$LOCALAPPDATA/Lexift')) {
    if ($installerScript.Contains($forbidden)) {
        throw "Rendered NSIS script contains an unsupported forward-slash path: $forbidden"
    }
}

if ($installerScript.Contains('"InstallLocation" "$\"$INSTDIR$\""')) {
    throw "Rendered NSIS script quotes InstallLocation; directory registry values must remain unquoted"
}

Write-Host "Verified Windows GUI executable metadata for $($item.Name) ($($item.Length) bytes)."
Write-Host "Verified rendered NSIS uninstall cleanup paths and failure handling."
