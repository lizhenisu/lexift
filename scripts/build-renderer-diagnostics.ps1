param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\target\renderer-diagnostics')
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$workspace = Join-Path $output 'workspace'
# Keep this build directory separate from the repository's own `target`. Sharing it
# let the copied workspace overwrite same-hash units in the main cache, so the next
# repository build could link a stale `lexift-platform`/`lexift-ui` artifact whose
# dep-info pointed at the copy instead of the real sources.
$build = Join-Path $output 'target'
New-Item -ItemType Directory -Force -Path $workspace, $build | Out-Null

# Build in a disposable workspace so renderer A/B runs never rewrite the user's manifests.
Copy-Item -LiteralPath (Join-Path $root 'Cargo.toml') -Destination $workspace -Force
Copy-Item -LiteralPath (Join-Path $root 'Cargo.lock') -Destination $workspace -Force
Copy-Item -LiteralPath (Join-Path $root 'rust-toolchain.toml') -Destination $workspace -Force
Copy-Item -LiteralPath (Join-Path $root 'crates') -Destination $workspace -Recurse -Force
Copy-Item -LiteralPath (Join-Path $root 'assets') -Destination $workspace -Recurse -Force

$manifestPath = Join-Path $workspace 'Cargo.toml'
$baseline = [System.IO.File]::ReadAllText($manifestPath)
if (-not $baseline.Contains('"renderer-femtovg"')) {
    throw 'The workspace Slint feature list no longer contains renderer-femtovg.'
}
# The normal build includes both renderers for startup selection. Remove the software
# fallback from diagnostic copies so each variant measures exactly one renderer.
$singleRendererBaseline = $baseline -replace '(?m)^\s*"renderer-software",\r?\n', ''

$variants = [ordered]@{
    'skia-opengl' = 'renderer-skia-opengl'
    'femtovg' = 'renderer-femtovg'
    'software' = 'renderer-software'
}

foreach ($variant in $variants.Keys) {
    $feature = $variants[$variant]
    [System.IO.File]::WriteAllText(
        $manifestPath,
        $singleRendererBaseline.Replace('"renderer-femtovg"', '"' + $feature + '"')
    )
    $env:CARGO_TARGET_DIR = $build
    $diagnosticFeature = if ($variant -eq 'software') { 'renderer-diagnostic-software' } else { 'renderer-diagnostic' }
    cargo build --release --manifest-path $manifestPath -p lexift-app --features $diagnosticFeature
    if ($LASTEXITCODE -ne 0) {
        throw "Release build failed for $variant"
    }
    $variantDirectory = Join-Path $output $variant
    New-Item -ItemType Directory -Force -Path $variantDirectory | Out-Null
    $exe = Join-Path $variantDirectory 'lexift.exe'
    Copy-Item -LiteralPath (Join-Path $build 'release\lexift.exe') -Destination $exe -Force
    Copy-Item -LiteralPath (Join-Path $root 'devdoc\resize-performance.md') -Destination (Join-Path $variantDirectory 'TESTING.md') -Force
    Compress-Archive -LiteralPath $exe, (Join-Path $variantDirectory 'TESTING.md') -DestinationPath (Join-Path $output "lexift-$variant.zip") -Force
    Get-Item -LiteralPath $exe | Select-Object FullName, Length
}

Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
[System.IO.File]::WriteAllText($manifestPath, $baseline)
