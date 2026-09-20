param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path
)

$ErrorActionPreference = "Stop"

if (-not $env:LEXIFT_SIGN_CERT_PATH) {
    Write-Host "Signing certificate is not configured; artifacts remain unsigned."
    exit 0
}

$signtool = (Get-Command signtool.exe -ErrorAction Stop).Source
foreach ($item in $Path) {
    if (-not (Test-Path -LiteralPath $item)) {
        throw "Signing target does not exist: $item"
    }

    $arguments = @(
        "sign", "/fd", "SHA256", "/td", "SHA256",
        "/tr", "http://timestamp.digicert.com",
        "/f", $env:LEXIFT_SIGN_CERT_PATH
    )
    if ($env:LEXIFT_SIGN_CERT_PASSWORD) {
        $arguments += @("/p", $env:LEXIFT_SIGN_CERT_PASSWORD)
    }
    $arguments += $item

    & $signtool @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for $item"
    }
}
