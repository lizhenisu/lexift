param([Parameter(ValueFromRemainingArguments = $true)][string[]]$AppArguments)
$ErrorActionPreference = 'Stop'
$projectDirectory = Split-Path -Parent $PSScriptRoot
$originalDeepSeekKey = [Environment]::GetEnvironmentVariable('LEXIFT_DEEPSEEK_API_KEY', 'Process')
try {
    if ($null -eq $originalDeepSeekKey) {
        $environmentFile = Join-Path $projectDirectory '.env'
        if (Test-Path -LiteralPath $environmentFile) {
            foreach ($line in [IO.File]::ReadLines($environmentFile)) {
                if ($line -match '^\s*(?:export\s+)?LEXIFT_DEEPSEEK_API_KEY\s*=\s*(.*?)\s*$') {
                    $value = $Matches[1]
                    if ($value.Length -ge 2 -and $value.StartsWith('"') -and $value.EndsWith('"')) { $value = $value.Substring(1, $value.Length - 2) }
                    elseif ($value.Length -ge 2 -and $value.StartsWith("'") -and $value.EndsWith("'")) { $value = $value.Substring(1, $value.Length - 2) }
                    else { $value = ($value -replace '\s+#.*$', '').Trim() }
                    [Environment]::SetEnvironmentVariable('LEXIFT_DEEPSEEK_API_KEY', $value, 'Process')
                    break
                }
            }
        }
    }
    Push-Location -LiteralPath $projectDirectory
    try { & cargo run -p lexift-app -- @AppArguments; $applicationExitCode = $LASTEXITCODE }
    finally { Pop-Location }
}
finally { [Environment]::SetEnvironmentVariable('LEXIFT_DEEPSEEK_API_KEY', $originalDeepSeekKey, 'Process') }
exit $applicationExitCode
