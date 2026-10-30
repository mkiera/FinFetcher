param(
    [Parameter(Mandatory = $true)][string]$Compiler,
    [Parameter(Mandatory = $true)][string]$PreviousPayload,
    [string]$RustPayload = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist/FinFetcher')
)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$previous = (Resolve-Path -LiteralPath $PreviousPayload).Path
$replacement = (Resolve-Path -LiteralPath $RustPayload).Path
if (-not (Test-Path -LiteralPath (Join-Path $previous '_internal/version.txt'))) { throw 'Previous payload must be a complete Python onedir application.' }
if (@(Get-ChildItem -LiteralPath $previous -Filter 'unins*.exe' -File).Count) { throw 'Use a build payload, not an installed application containing an existing uninstaller.' }
if (-not (Test-Path -LiteralPath (Join-Path $replacement 'FinFetcher.exe'))) { throw 'Rust payload must contain FinFetcher.exe.' }
$expectedVersion = (Get-Content -LiteralPath (Join-Path $replacement 'version.txt') -Raw).Trim()
$root = Join-Path ([IO.Path]::GetTempPath()) ('FinFetcher-python-migration-' + [Guid]::NewGuid().ToString('N'))
$install = Join-Path $root 'custom-install-folder'
$roaming = Join-Path $root 'roaming'
$state = Join-Path $roaming 'FinFetcher'
$updates = Join-Path $state 'updates'
$name = 'FinFetcherMigration-' + [Guid]::NewGuid().ToString('N')
$installerBase = $name + '-Setup'
$packageId = '{{' + [Guid]::NewGuid().ToString() + '}'
New-Item -ItemType Directory -Path $install, $updates | Out-Null
Copy-Item -Path (Join-Path $previous '*') -Destination $install -Recurse
$configuration = '{"migration_probe":"preserve","auto_check_updates":false,"auto_update_ytdlp":false,"update_channel":"prerelease","container":"mkv","log_to_file":false}'
[IO.File]::WriteAllText((Join-Path $state 'config.json'), $configuration)
[IO.File]::WriteAllText((Join-Path $install 'user-video.mp4'), 'user-owned')
$numeric = ($expectedVersion -split '[-+]')[0] + '.0'
$uninstallMarker = Join-Path $root 'uninstall-finished.txt'
$definitions = @('/Q', "/O$root", "/F$installerBase", "/DAppName=$name", "/DAppId=$packageId", "/DStorageId=$name", "/DAppDataDir=$state", "/DAppSourceDir=$replacement", "/DVersionNumeric=$numeric", "/DAppVersion=$expectedVersion", "/DTestStateDir=$state", "/DTestUninstallMarker=$uninstallMarker", '/DTestNoIcons')
& $Compiler @definitions (Join-Path $project 'installer.iss')
if ($LASTEXITCODE -ne 0) { throw 'Migration installer compilation failed.' }
$registration = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$($packageId.Substring(1))_is1"
New-Item -Path $registration -Force | Out-Null
New-ItemProperty -Path $registration -Name 'Inno Setup: App Path' -Value $install -PropertyType String -Force | Out-Null
New-ItemProperty -Path $registration -Name 'Inno Setup: User' -Value $env:USERNAME -PropertyType String -Force | Out-Null
New-ItemProperty -Path $registration -Name 'Inno Setup: Selected Tasks' -Value '' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $registration -Name 'InstallLocation' -Value $install -PropertyType String -Force | Out-Null

function Test-Processes {
    @(Get-CimInstance Win32_Process -Filter "Name='FinFetcher.exe'" | Where-Object {
        $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath).Equals((Join-Path $install 'FinFetcher.exe'), [StringComparison]::OrdinalIgnoreCase)
    })
}

function Test-InstallerProcesses {
    @(Get-CimInstance Win32_Process -Filter "Name='$installerBase.exe' OR Name='$installerBase.tmp'" | Where-Object {
        ($_.ExecutablePath -and $_.ExecutablePath.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) -or
        ($_.CommandLine -and $_.CommandLine.IndexOf($root, [StringComparison]::OrdinalIgnoreCase) -ge 0)
    })
}

$old = $null
try {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $install 'FinFetcher.exe'
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $start.EnvironmentVariables['APPDATA'] = $roaming
    $start.EnvironmentVariables['LOCALAPPDATA'] = Join-Path $root 'local'
    $start.EnvironmentVariables['FINFETCHER_DATA_DIR'] = $state
    $start.EnvironmentVariables['WEBVIEW2_USER_DATA_FOLDER'] = Join-Path $state 'webview2'
    $start.EnvironmentVariables['PYWEBVIEW_LOG'] = 'error'
    $old = [Diagnostics.Process]::Start($start)
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $address = $null
    while ([DateTime]::UtcNow -lt $deadline -and -not $address) {
        $running = Test-Processes
        foreach ($process in $running) {
            $connections = Get-NetTCPConnection -OwningProcess $process.ProcessId -State Listen -ErrorAction SilentlyContinue
            foreach ($connection in $connections) {
                $candidate = "http://127.0.0.1:$($connection.LocalPort)"
                try {
                    $info = Invoke-RestMethod -Uri "$candidate/api/update/settings" -TimeoutSec 2
                    if ($info.current_version -and $info.can_self_update) { $address = $candidate; break }
                } catch { }
            }
            if ($address) { break }
        }
        if (-not $address) { Start-Sleep -Milliseconds 200 }
    }
    if (-not $address) { throw "The isolated Python application's update API did not start. Files: $root" }
    if ($info.update_channel -ne 'prerelease' -or $info.auto_check_updates -ne $false) { throw 'The original application did not load isolated update settings.' }
    $oldProcesses = @(Test-Processes | ForEach-Object { [int]$_.ProcessId })
    $oldVersion = (Invoke-RestMethod -Uri "$address/api/update/settings").current_version
    $before = Get-Content -LiteralPath (Join-Path $state 'config.json') -Raw | ConvertFrom-Json
    $installerPath = Join-Path $updates ($installerBase + '.exe')
    Copy-Item -LiteralPath (Join-Path $root ($installerBase + '.exe')) -Destination $installerPath
    $body = @{path = $installerPath} | ConvertTo-Json -Compress
    $handoff = Invoke-RestMethod -Uri "$address/api/update/apply" -Method Post -ContentType 'application/json' -Body $body -TimeoutSec 15
    if (-not $handoff.success) { throw "The original Python updater refused its installer: $($handoff | ConvertTo-Json -Compress)" }
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    $relaunched = $null
    while ([DateTime]::UtcNow -lt $deadline) {
        $remaining = Test-Processes
        $previousRunning = @($remaining | Where-Object { $oldProcesses -contains [int]$_.ProcessId })
        $newRunning = @($remaining | Where-Object { $oldProcesses -notcontains [int]$_.ProcessId })
        if ($previousRunning.Count -eq 0 -and $newRunning.Count -gt 0 -and
            -not (Test-Path -LiteralPath (Join-Path $install '_internal')) -and
            (Test-Path -LiteralPath (Join-Path $install 'version.txt'))) {
            $relaunched = $newRunning[0]
            break
        }
        Start-Sleep -Milliseconds 200
    }
    if (-not $relaunched) { throw "The Rust application did not replace and relaunch after the Python updater handoff. Log: $state/update.log" }
    Start-Sleep -Seconds 2
    if (-not (Get-Process -Id $relaunched.ProcessId -ErrorAction SilentlyContinue)) { throw 'The relaunched Rust application exited immediately.' }
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (@(Test-InstallerProcesses).Count) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'The installer did not finish after relaunching the Rust application.' }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath (Join-Path $state 'webview2'))) { throw 'Migration did not use the isolated WebView2 data folder.' }
    if ((Get-Content -LiteralPath (Join-Path $install 'version.txt') -Raw).Trim() -ne $expectedVersion) { throw 'The installed version does not match the Rust payload.' }
    $installedHash = (Get-FileHash -LiteralPath (Join-Path $install 'FinFetcher.exe')).Hash
    if ($installedHash -ne (Get-FileHash -LiteralPath (Join-Path $replacement 'FinFetcher.exe')).Hash) { throw 'The installed executable does not match the Rust build.' }
    $after = Get-Content -LiteralPath (Join-Path $state 'config.json') -Raw | ConvertFrom-Json
    foreach ($property in $before.PSObject.Properties) {
        if (($after.($property.Name) | ConvertTo-Json -Compress -Depth 20) -ne ($property.Value | ConvertTo-Json -Compress -Depth 20)) {
            throw "Migration changed existing setting: $($property.Name)"
        }
    }
    if ((Get-Content -LiteralPath (Join-Path $install 'user-video.mp4') -Raw) -ne 'user-owned') { throw 'Migration changed a user file.' }
    if ((Get-ItemProperty -Path $registration).'Inno Setup: App Path' -ne $install) { throw 'Migration changed the installation directory.' }
    Write-Output "Passed: original Python $oldVersion updater to Rust $expectedVersion, old-process exit, exact executable replacement, relaunch, custom location, existing settings, and user files."
} finally {
    foreach ($process in (Test-InstallerProcesses)) { Stop-Process -Id $process.ProcessId -ErrorAction SilentlyContinue }
    foreach ($process in (Test-Processes)) { Stop-Process -Id $process.ProcessId -ErrorAction SilentlyContinue }
    foreach ($process in (Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object {
        $_.CommandLine -and $_.CommandLine.IndexOf($root, [StringComparison]::OrdinalIgnoreCase) -ge 0
    })) { Stop-Process -Id $process.ProcessId -ErrorAction SilentlyContinue }
    $uninstaller = Join-Path $install 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $cleanup = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -WindowStyle Hidden -PassThru
        if (-not $cleanup.WaitForExit(30000)) { Stop-Process -Id $cleanup.Id }
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        while (-not (Test-Path -LiteralPath $uninstallMarker)) {
            if ([DateTime]::UtcNow -gt $deadline) { throw "The isolated uninstaller did not finish cleanup. Files: $root" }
            Start-Sleep -Milliseconds 20
        }
    }
    if (Test-Path -LiteralPath $registration) { Remove-Item -LiteralPath $registration -Recurse }
    Write-Output "Python migration test files: $root"
}
