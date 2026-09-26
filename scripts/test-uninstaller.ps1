param([Parameter(Mandatory = $true)][string]$Compiler)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$root = Join-Path ([IO.Path]::GetTempPath()) ('FinFetcher-uninstall-test-' + [Guid]::NewGuid().ToString('N'))
$payload = Join-Path $root 'payload'
New-Item -ItemType Directory -Path $payload | Out-Null
& rustc --edition 2021 (Join-Path $project 'tests/fixtures/update_app.rs') -o (Join-Path $payload 'FinFetcher.exe')
if ($LASTEXITCODE -ne 0) { throw 'Uninstaller fixture compilation failed.' }
[IO.File]::WriteAllText((Join-Path $payload 'version.txt'), 'new')
$managed = @('ffmpeg', 'ytdlp', 'ytdlp-bin', 'deno', 'tools', 'cache', 'webview2', 'cookies-Ab01z9')
$preserved = @('user-video.mp4', 'cookies-personal/notes.txt', 'other/user-owned.txt')
$silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/NOICONS', '/TASKS=')
foreach ($removeData in @($false, $true)) {
    $scenario = $(if ($removeData) { 'remove-data' } else { 'retain-data' })
    $scenarioRoot = Join-Path $root $scenario
    $install = Join-Path $scenarioRoot 'install'
    $state = Join-Path $scenarioRoot 'state'
    $name = 'FinFetcherUninstall-' + [Guid]::NewGuid().ToString('N')
    $packageId = '{{' + [Guid]::NewGuid().ToString() + '}'
    New-Item -ItemType Directory -Path $state -Force | Out-Null
    foreach ($directory in $managed + @('updates', 'cookies')) {
        New-Item -ItemType Directory -Path (Join-Path $state $directory) | Out-Null
        [IO.File]::WriteAllText((Join-Path $state "$directory/test.bin"), 'managed')
    }
    [IO.File]::WriteAllText((Join-Path $state 'config.json'), '{"preserve":true}')
    foreach ($file in $preserved) {
        New-Item -ItemType Directory -Path (Split-Path -Parent (Join-Path $state $file)) -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $state $file), 'user-owned')
    }
    $marker = Join-Path $scenarioRoot 'uninstall-finished.txt'
    $definitions = @('/Q', "/O$scenarioRoot", '/FUninstallTest-Setup', "/DAppName=$name", "/DAppId=$packageId", "/DStorageId=$name", "/DAppDataDir=$state", "/DAppSourceDir=$payload", "/DTestUninstallMarker=$marker", '/DVersionNumeric=1.2.10.0', '/DAppVersion=1.2.10', '/DSkipWebView2')
    if ($removeData) { $definitions += '/DTestRemoveUserData' }
    & $Compiler @definitions (Join-Path $project 'installer.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Uninstaller test setup compilation failed.' }
    $initial = Start-Process -FilePath (Join-Path $scenarioRoot 'UninstallTest-Setup.exe') -ArgumentList ($silent + @('/DIR="' + $install + '"')) -WindowStyle Hidden -PassThru
    if (-not $initial.WaitForExit(30000) -or $initial.ExitCode -ne 0) { throw 'Uninstaller test installation failed.' }
    $uninstall = Start-Process -FilePath (Join-Path $install 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -WindowStyle Hidden -PassThru
    if (-not $uninstall.WaitForExit(30000) -or $uninstall.ExitCode -ne 0) { throw 'Uninstaller test removal failed.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $marker)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Uninstaller did not finish its data cleanup.' }
        Start-Sleep -Milliseconds 20
    }
    foreach ($path in $managed + @('config.json')) {
        if ((Test-Path -LiteralPath (Join-Path $state $path)) -eq $removeData) { throw "Incorrect $scenario behavior for $path" }
    }
    foreach ($path in @('updates', 'cookies')) {
        if (Test-Path -LiteralPath (Join-Path $state $path)) { throw "Uninstall retained temporary $path data." }
    }
    foreach ($file in $preserved) {
        if ((Get-Content -LiteralPath (Join-Path $state $file) -Raw) -ne 'user-owned') { throw "Uninstall changed unrelated user file $file" }
    }
    Write-Output "Passed: $scenario, owned-directory list, temporary cookie directories, and unrelated user files."
}
Write-Output "Uninstaller test files: $root"
