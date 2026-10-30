$ErrorActionPreference = 'Stop'
& $PSScriptRoot/build.ps1 -PortableOnly
if (-not $?) { throw 'Portable application build failed.' }
$project = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -LiteralPath (Join-Path $project 'build/version/version.txt') -Raw).Trim()
$destination = Join-Path $project "dist_installer/FinFetcher_v$version.zip"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
Compress-Archive -Path (Join-Path $project 'dist/FinFetcher/*') -DestinationPath $destination -Force
Write-Output "Portable archive: $destination"
