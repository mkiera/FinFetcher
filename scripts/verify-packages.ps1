param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$bundled = (Get-Content -LiteralPath (Join-Path $project 'dist/FinFetcher/version.txt') -Raw).Trim()
if ($bundled -ne $Version) { throw "Bundled version mismatch: $bundled" }
foreach ($package in @('dist/FinFetcher/FinFetcher.exe', 'dist_legacy/FinFetcher-Legacy.exe', 'dist_installer/FinFetcher-Setup.exe')) {
    $path = Join-Path $project $package
    $productVersion = (Get-Item -LiteralPath $path).VersionInfo.ProductVersion.Trim()
    if ($productVersion -ne $Version) { throw "Version mismatch in ${package}: $productVersion" }
}
$appHash = (Get-FileHash -LiteralPath (Join-Path $project 'dist/FinFetcher/FinFetcher.exe')).Hash
$legacyHash = (Get-FileHash -LiteralPath (Join-Path $project 'dist_legacy/FinFetcher-Legacy.exe')).Hash
if ($appHash -ne $legacyHash) { throw 'Legacy asset differs from the application executable.' }
