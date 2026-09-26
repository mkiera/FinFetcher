param([Parameter(Mandatory = $true)][string]$Compiler)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$root = Join-Path ([IO.Path]::GetTempPath()) ('FinFetcher-install-test-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$fixture = Join-Path $root 'fixture.exe'
& rustc --edition 2021 (Join-Path $project 'tests/fixtures/update_app.rs') -o $fixture
if ($LASTEXITCODE -ne 0) { throw 'Installer fixture compilation failed.' }
$previousFixture = Join-Path $root 'fixture-old.exe'
& rustc --edition 2021 --cfg old_payload (Join-Path $project 'tests/fixtures/update_app.rs') -o $previousFixture
if ($LASTEXITCODE -ne 0) { throw 'Previous installer fixture compilation failed.' }
$previousHash = (Get-FileHash -LiteralPath $previousFixture).Hash

function Wait-File([string]$Path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (Test-Path -LiteralPath $Path)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw "File did not appear: $Path" }
        Start-Sleep -Milliseconds 20
    }
}

foreach ($scenario in @('running', 'blocked', 'rollback', 'prerequisite')) {
    $scenarioRoot = Join-Path $root $scenario
    $payload = Join-Path $scenarioRoot 'payload'
    $install = Join-Path $scenarioRoot 'custom-install-folder'
    $name = 'FinFetcherFixture-' + [Guid]::NewGuid().ToString('N')
    $packageId = '{{' + [Guid]::NewGuid().ToString() + '}'
    New-Item -ItemType Directory -Path (Join-Path $payload '_internal') -Force | Out-Null
    Copy-Item -LiteralPath $previousFixture -Destination (Join-Path $payload ($name + '.exe'))
    [IO.File]::WriteAllText((Join-Path $payload 'version.txt'), $(if ($scenario -eq 'blocked') { 'blocked' } else { 'old' }))
    [IO.File]::WriteAllText((Join-Path $payload '_internal/runtime.pyd'), 'old-runtime')
    $definitions = @('/Q', "/O$scenarioRoot", "/DAppName=$name", "/DAppExeName=$name.exe", "/DAppId=$packageId", "/DStorageId=$name", "/DAppSourceDir=$payload", '/DVersionNumeric=1.2.9.0', '/DAppVersion=1.2.9', '/DSkipWebView2')
    & $Compiler @definitions '/FInitial-Setup' (Join-Path $project 'installer.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Initial fixture installer compilation failed.' }
    $silent = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CLOSEAPPLICATIONS', '/NORESTARTAPPLICATIONS', '/SP-', '/NOICONS', '/TASKS=')
    $old = $null
    $setup = $null
    try {
        $initialArgs = $silent + @('/DIR="' + $install + '"', '/LOG="' + (Join-Path $scenarioRoot 'initial.log') + '"')
        $initial = Start-Process -FilePath (Join-Path $scenarioRoot 'Initial-Setup.exe') -ArgumentList $initialArgs -WindowStyle Hidden -PassThru
        if (-not $initial.WaitForExit(30000) -or $initial.ExitCode -ne 0) { throw 'Initial fixture installation failed.' }
        Wait-File (Join-Path $install 'started.txt')
        $old = Get-Process -Id ([int](Get-Content -LiteralPath (Join-Path $install 'started.txt') -Raw))
        $settingsPath = Join-Path $env:APPDATA "$name/config.json"
        New-Item -ItemType Directory -Path (Split-Path -Parent $settingsPath) -Force | Out-Null
        [IO.File]::WriteAllText($settingsPath, '{"preserve":true,"update_channel":"prerelease"}')
        [IO.File]::WriteAllText((Join-Path $install 'user-video.mp4'), 'user-owned')
        [IO.File]::WriteAllText((Join-Path $payload 'version.txt'), 'new')
        Copy-Item -LiteralPath $fixture -Destination (Join-Path $payload ($name + '.exe')) -Force
        $oldRuntime = (Resolve-Path -LiteralPath (Join-Path $payload '_internal')).Path
        if (-not $oldRuntime.StartsWith((Resolve-Path -LiteralPath $root).Path + [IO.Path]::DirectorySeparatorChar)) { throw 'Invalid fixture runtime path.' }
        Remove-Item -LiteralPath $oldRuntime -Recurse -Force
        $definitions = @('/Q', "/O$scenarioRoot", "/DAppName=$name", "/DAppExeName=$name.exe", "/DAppId=$packageId", "/DStorageId=$name", "/DAppSourceDir=$payload", '/DVersionNumeric=1.2.10.0', '/DAppVersion=1.2.10')
        if ($scenario -eq 'prerequisite') {
            $definitions += @('/DTestRuntimeMissing', "/DWebView2Bootstrapper=$fixture")
        } else {
            $definitions += '/DSkipWebView2'
        }
        if ($scenario -eq 'rollback') { $definitions += "/DTestCopyFailure=$scenarioRoot/missing-payload.bin" }
        & $Compiler @definitions '/FUpdate-Setup' (Join-Path $project 'installer.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Update fixture installer compilation failed.' }
        $updateArgs = $silent + @('/LOG="' + (Join-Path $install 'update.log') + '"')
        $setup = Start-Process -FilePath (Join-Path $scenarioRoot 'Update-Setup.exe') -ArgumentList $updateArgs -WindowStyle Hidden -PassThru
        if (-not $setup.WaitForExit(55000)) { throw "Installer timed out in $scenario." }
        if ($scenario -eq 'running') {
            if ($setup.ExitCode -ne 0) { throw "Running-app upgrade failed. Log: $install/update.log" }
            if (-not $old.WaitForExit(5000)) { throw 'Old application remained running.' }
            Wait-File (Join-Path $install 'relaunched.txt')
            if ((Get-Content -LiteralPath (Join-Path $install 'relaunched.txt') -Raw) -ne 'new') { throw 'The old application was relaunched.' }
            if ((Get-Content -LiteralPath (Join-Path $install 'executable-build.txt') -Raw) -ne 'new') { throw 'The old executable was relaunched.' }
            if (Test-Path -LiteralPath (Join-Path $install '_internal')) { throw 'Python runtime remained after successful migration.' }
        } else {
            if ($setup.ExitCode -eq 0) { throw "The $scenario upgrade unexpectedly succeeded." }
            if (-not (Test-Path -LiteralPath (Join-Path $install ($name + '.exe')))) { throw 'Failed upgrade removed the previous executable.' }
            if ($scenario -eq 'blocked' -and $old.HasExited) { throw 'Blocked update terminated the old application.' }
            if (-not $old.HasExited) { Stop-Process -Id $old.Id }
            if ((Get-Content -LiteralPath (Join-Path $install '_internal/runtime.pyd') -Raw) -ne 'old-runtime') { throw 'Failed upgrade did not restore the previous runtime.' }
            if ((Get-Content -LiteralPath (Join-Path $install 'version.txt') -Raw) -eq 'new') { throw 'Failed upgrade left the new version stamp.' }
            if ((Get-FileHash -LiteralPath (Join-Path $install ($name + '.exe'))).Hash -ne $previousHash) { throw 'Failed upgrade did not restore the old executable bytes.' }
            Remove-Item -LiteralPath (Join-Path $install 'started.txt')
            $old = Start-Process -FilePath (Join-Path $install ($name + '.exe')) -WindowStyle Hidden -PassThru
            Wait-File (Join-Path $install 'started.txt')
        }
        if (Test-Path -LiteralPath (Join-Path $install '_internal.old')) { throw 'Installation left a runtime backup behind.' }
        if (Test-Path -LiteralPath (Join-Path $install ($name + '.exe.previous'))) { throw 'Installation left an executable backup behind.' }
        if ((Get-Content -LiteralPath $settingsPath -Raw) -ne '{"preserve":true,"update_channel":"prerelease"}') { throw 'Update changed user settings.' }
        if ((Get-Content -LiteralPath (Join-Path $install 'user-video.mp4') -Raw) -ne 'user-owned') { throw 'Update changed user files.' }
        Write-Output "Passed: $scenario, installation location, settings, and user files."
    } finally {
        if ($setup -and -not $setup.HasExited) { Stop-Process -Id $setup.Id }
        if ($old -and -not $old.HasExited) { Stop-Process -Id $old.Id }
        $uninstaller = Join-Path $install 'unins000.exe'
        if (Test-Path -LiteralPath $uninstaller) {
            $cleanup = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -WindowStyle Hidden -PassThru
            if (-not $cleanup.WaitForExit(30000)) { Stop-Process -Id $cleanup.Id }
        }
    }
}
Write-Output "Installer test files: $root"
