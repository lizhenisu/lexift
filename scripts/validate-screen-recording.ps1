param(
    [Parameter(Mandatory)][string]$TestExe,
    [Parameter(Mandatory)][string]$EvidenceDirectory,
    [Parameter(Mandatory)][int[]]$Region,
    [int]$Rounds = 20,
    [int]$DynamicMilliseconds = 6000,
    [int]$StaticMilliseconds = 6000,
    [int]$PauseMilliseconds = 2000,
    [int]$ResumeMilliseconds = 2000,
    [int]$Fps = 30,
    [switch]$Hardware,
    [int]$CancelMilliseconds = -1,
    [int]$StallMilliseconds = 0,
    [string]$ExpectedError = '',
    [string]$Fault = '',
    [switch]$NativeControls,
    [switch]$Mixed,
    [switch]$CursorPath,
    [switch]$ProbeReopen,
    [switch]$RecoverLast
)
# Explicit gate driver: only synthetic desktop content and owned output paths.
$ErrorActionPreference = 'Stop'
if ($Region.Count -ne 4) { throw 'Region requires physical left, top, right, bottom' }
if (Test-Path -LiteralPath $EvidenceDirectory) { throw 'Use a new evidence directory; previous results are preserved' }
$gateExe = (Resolve-Path -LiteralPath $TestExe).Path
New-Item -ItemType Directory -Path $EvidenceDirectory | Out-Null
$gateFolder = (Resolve-Path -LiteralPath $EvidenceDirectory).Path
$spec = @{
    dynamic_ms = $DynamicMilliseconds; static_ms = $StaticMilliseconds
    pause_ms = $PauseMilliseconds; resume_ms = $ResumeMilliseconds
    fps = $Fps; hardware = [bool]$Hardware; stall_ms = $StallMilliseconds
}
if ($CancelMilliseconds -ge 0) { $spec.cancel_ms = $CancelMilliseconds }
if ($Fault) { $spec.fault = $Fault }
$spec.native_controls = [bool]$NativeControls
$spec.cursor_path = [bool]$CursorPath
$plan = @{roi = $Region; rounds = $Rounds; spec = $spec; mixed = [bool]$Mixed; probe_reopen = [bool]$ProbeReopen; recover_last = [bool]$RecoverLast}
if ($ExpectedError) { $plan.expected_error = $ExpectedError }
$plan | ConvertTo-Json -Depth 4 |
    Set-Content -LiteralPath (Join-Path $gateFolder 'plan.json') -Encoding utf8NoBOM
Get-FileHash -LiteralPath $gateExe -Algorithm SHA256 | ConvertTo-Json |
    Set-Content -LiteralPath (Join-Path $gateFolder 'build.json') -Encoding utf8NoBOM
$previousDirectory = $env:LEXIFT_RECORDING_GATE_DIR
try {
    $env:LEXIFT_RECORDING_GATE_DIR = $gateFolder
    & $gateExe windows::recording_gate::native_recording_matrix --exact --ignored --nocapture 2>&1 |
        Tee-Object -FilePath (Join-Path $gateFolder 'native-test.log')
    if ($LASTEXITCODE -ne 0) { throw "Native gate failed: $LASTEXITCODE; evidence retained in $gateFolder" }
} finally { $env:LEXIFT_RECORDING_GATE_DIR = $previousDirectory }
