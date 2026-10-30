param([switch]$Prepared, [switch]$SkipTests, [switch]$PortableOnly)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
Push-Location $project
try {
    if (-not (Test-Path -LiteralPath 'node_modules/@tauri-apps/cli')) {
        & npm.cmd ci
        if ($LASTEXITCODE -ne 0) { throw 'Node dependency installation failed.' }
    }
    if (-not $Prepared) {
        & node scripts/build-identity.mjs
        if ($LASTEXITCODE -ne 0) { throw 'Version preparation failed.' }
    }
    $identity = Get-Content -LiteralPath 'build_info.json' -Raw | ConvertFrom-Json
    $env:FINFETCHER_BUILD_VERSION = $identity.version
    & npm.cmd run build:frontend
    if ($LASTEXITCODE -ne 0) { throw 'Frontend staging failed.' }
    if (-not $SkipTests) {
        & npm.cmd test
        if ($LASTEXITCODE -ne 0) { throw 'Frontend or release tests failed.' }
        & cargo test --locked --manifest-path src-tauri/Cargo.toml
        if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed.' }
    }
    & npm.cmd run tauri -- build --no-bundle --config src-tauri/build-config.json
    if ($LASTEXITCODE -ne 0) { throw 'Application build failed.' }
    & node scripts/stage-payload.mjs
    if ($LASTEXITCODE -ne 0) { throw 'Application staging failed.' }
    if ($PortableOnly) { return }
    $compiler = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
    if (-not $compiler) {
        $candidates = @("${env:ProgramFiles(x86)}\Inno Setup *\ISCC.exe", "$env:ProgramFiles\Inno Setup *\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup *\ISCC.exe")
        $compiler = Get-ChildItem -Path $candidates -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $compiler) { throw 'Inno Setup is required to build FinFetcher-Setup.exe.' }
    & $PSScriptRoot/prepare-webview2.ps1
    if (-not $?) { throw 'WebView2 bootstrapper preparation failed.' }
    $numeric = ($identity.version -split '[-+]')[0] + '.0'
    & $compiler /Q "/DVersionNumeric=$numeric" "/DAppVersion=$($identity.version)" installer.iss
    if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
    & $PSScriptRoot/verify-packages.ps1 -Version $identity.version
    if (-not $?) { throw 'Packaged version verification failed.' }
    Write-Output "Installer: $project\dist_installer\FinFetcher-Setup.exe"
} finally {
    Pop-Location
}
