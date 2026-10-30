param(
    [Parameter(Mandatory)][string]$TestExe,
    [Parameter(Mandatory)][string]$EvidenceRoot,
    [Parameter(Mandatory)][string]$Python,
    [Parameter(Mandatory)][string]$Ffmpeg,
    [Parameter(Mandatory)][string]$Ffprobe
)
$ErrorActionPreference = 'Stop'
if(Test-Path -LiteralPath $EvidenceRoot){throw 'Use a new final evidence directory'}
New-Item -ItemType Directory -Path $EvidenceRoot | Out-Null
$exe=(Resolve-Path -LiteralPath $TestExe).Path
$root=(Resolve-Path -LiteralPath $EvidenceRoot).Path
$driver=Join-Path $PSScriptRoot 'validate-screen-recording.ps1'
$verifier=Join-Path $PSScriptRoot 'validate-screen-recording.py'
$folders=[System.Collections.Generic.List[string]]::new()
$savedEncoder=$env:LEXIFT_RECORDING_GATE_ENCODER
$savedIndex=$env:LEXIFT_RECORDING_GATE_DEVICE_INDEX
try {
    foreach($backend in @('software','nvidia')) {
        if($backend -eq 'nvidia'){$env:LEXIFT_RECORDING_GATE_ENCODER='60F44560-5A20-4857-BFEF-D29773CB8040'}
        else {Remove-Item Env:LEXIFT_RECORDING_GATE_ENCODER -ErrorAction SilentlyContinue}
        Remove-Item Env:LEXIFT_RECORDING_GATE_DEVICE_INDEX -ErrorAction SilentlyContinue
        foreach($display in @(@{name='125';region=@(320,200,2240,1280)},@{name='100';region=@(2560,0,4480,1080)})) {
            $folder=Join-Path $root "$backend-$($display.name)"
            & $driver -TestExe $exe -EvidenceDirectory $folder -Region $display.region -Rounds 20 -Hardware:($backend -eq 'nvidia') -ProbeReopen -DynamicMilliseconds 3000 -StaticMilliseconds 1000 -PauseMilliseconds 500 -ResumeMilliseconds 1000
            $folders.Add($folder)
        }
    }
    Remove-Item Env:LEXIFT_RECORDING_GATE_ENCODER -ErrorAction SilentlyContinue
    $folder=Join-Path $root 'software-soak'
    & $driver -TestExe $exe -EvidenceDirectory $folder -Region 320,200,2240,1280 -Rounds 1 -DynamicMilliseconds 597000 -StaticMilliseconds 1000 -PauseMilliseconds 1000 -ResumeMilliseconds 2000
    $folders.Add($folder)
    foreach($display in @(@{name='125';region=@(320,200,2240,1280)},@{name='100';region=@(2560,0,4480,1080)})) {
        $folder=Join-Path $root "input-$($display.name)"
        & $driver -TestExe $exe -EvidenceDirectory $folder -Region $display.region -Rounds 5 -NativeControls -CursorPath -DynamicMilliseconds 2000 -StaticMilliseconds 1000 -PauseMilliseconds 500 -ResumeMilliseconds 1000
        $folders.Add($folder)
    }
    # Decode only after all timings: no competing build or decoder during benchmarks.
    $failed=[System.Collections.Generic.List[string]]::new()
    foreach($folder in $folders) {
        & $Python $verifier $folder --ffmpeg $Ffmpeg --ffprobe $Ffprobe > (Join-Path $folder 'decode.log')
        if($LASTEXITCODE -ne 0){$failed.Add($folder)}
    }
    & $Python (Join-Path $PSScriptRoot 'summarize-screen-recording.py') $root > (Join-Path $root 'qualification.log')
    $summaryExit=$LASTEXITCODE
    $qualification=Get-Content (Join-Path $root 'qualification.json') -Raw | ConvertFrom-Json
    @{decodeFailures=@($failed);folders=@($folders);hardwareQualified=$qualification.hardware_qualified;passed=$qualification.software_gate_passed} | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $root 'matrix.json') -Encoding utf8NoBOM
    if($summaryExit -ne 0){throw 'Mandatory software gate failed; evidence preserved'}
} finally {
    $env:LEXIFT_RECORDING_GATE_ENCODER=$savedEncoder
    $env:LEXIFT_RECORDING_GATE_DEVICE_INDEX=$savedIndex
}
