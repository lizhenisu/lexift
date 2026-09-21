param(
    [string]$Executable = "target/release/lexift.exe",
    [string]$RenderedInstallerScript = "dist/installer/.cargo-packager/nsis/x64/installer.nsi",
    [string]$InstalledExecutable = ""
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
if ([Text.Encoding]::ASCII.GetString($bytes).Contains("GetWindowSubclass")) {
    throw "Executable imports unsupported GetWindowSubclass entry point"
}

$portableExecutable = Get-ChildItem -Path "dist/portable/Lexift-*-windows-x86_64/lexift.exe" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if ($portableExecutable) {
    $releaseHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
    $portableHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $portableExecutable.FullName).Hash
    if ($releaseHash -ne $portableHash) {
        throw "Portable executable does not match the verified release executable"
    }
}

if ($InstalledExecutable) {
    if (-not (Test-Path -LiteralPath $InstalledExecutable)) {
        throw "Missing installed executable: $InstalledExecutable"
    }
    $releaseHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
    $installedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $InstalledExecutable).Hash
    if ($releaseHash -ne $installedHash) {
        throw "Installed executable does not match the verified release executable"
    }
}

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
    'SetErrorLevel 1',
    'Delete "$INSTDIR\lexift.ico"',
    'Delete "$INSTDIR\lexift-${VERSION}.ico"',
    '${StrLoc} $6 $5 "\appdata\local\packages\" ">"',
    'CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWNORMAL',
    'CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe" "" "$INSTDIR\${MAINBINARYNAME}.exe" 0 SW_SHOWNORMAL',
    'SHChangeNotify(i 0x00000004, i 0x00001005',
    'SHChangeNotify(i 0x00000002, i 0x00001005',
    'SHChangeNotify(i 0x08000000, i 0x00001000'
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

$runningLexift = @(Get-Process -Name lexift -ErrorAction SilentlyContinue)
if ($runningLexift.Count -ne 0) {
    throw "Close all running Lexift processes before release startup verification"
}

$process = $null
try {
    $process = Start-Process -FilePath $item.FullName -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $observedTitle = ""
    while ([DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 200
        $process.Refresh()
        if ($process.HasExited) {
            throw "Release executable exited before showing its main window (exit code: $($process.ExitCode))"
        }
        $observedTitle = $process.MainWindowTitle
        if ($observedTitle -eq "Lexift") { break }
        if ($observedTitle) {
            throw "Release executable showed an unexpected startup window: $observedTitle"
        }
    }
    if ($observedTitle -ne "Lexift") {
        throw "Release executable did not show the Lexift main window within 10 seconds"
    }
}
finally {
    if ($process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit(5000) | Out-Null
    }
}

Write-Host "Verified Windows GUI executable metadata for $($item.Name) ($($item.Length) bytes)."
Write-Host "Verified release executable startup and portable/installed executable hashes."
Write-Host "Verified rendered NSIS cleanup, shortcut icon, and Shell refresh behavior."
