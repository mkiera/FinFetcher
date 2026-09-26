$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$directory = Join-Path $project 'build/webview2'
$bootstrapper = Join-Path $directory 'MicrosoftEdgeWebView2Setup.exe'
New-Item -ItemType Directory -Force -Path $directory | Out-Null
if (-not (Test-Path -LiteralPath $bootstrapper)) {
    $partial = Join-Path $directory 'MicrosoftEdgeWebView2Setup.part'
    Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $partial
    $signature = Get-AuthenticodeSignature -LiteralPath $partial
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
        throw 'The WebView2 bootstrapper does not have a valid Microsoft signature.'
    }
    Move-Item -LiteralPath $partial -Destination $bootstrapper
}
$signature = Get-AuthenticodeSignature -LiteralPath $bootstrapper
if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
    throw 'The cached WebView2 bootstrapper does not have a valid Microsoft signature.'
}
Write-Output "WebView2 bootstrapper: $bootstrapper"
