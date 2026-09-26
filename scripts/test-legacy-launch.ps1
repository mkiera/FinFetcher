param([string]$Executable = (Join-Path (Split-Path -Parent $PSScriptRoot) 'dist_legacy/FinFetcher-Legacy.exe'))
$ErrorActionPreference = 'Stop'
$source = (Resolve-Path -LiteralPath $Executable).Path
$root = Join-Path ([IO.Path]::GetTempPath()) ('FinFetcher-legacy-launch-' + [Guid]::NewGuid().ToString('N'))
$install = Join-Path $root 'renamed-legacy-application'
$state = Join-Path $root 'state'
New-Item -ItemType Directory -Path $install, $state | Out-Null
$renamed = Join-Path $install 'FinFetcher.exe'
Copy-Item -LiteralPath $source -Destination $renamed
$configuration = '{"legacy_probe":"preserve","auto_check_updates":false,"auto_update_ytdlp":false,"update_channel":"prerelease","container":"mkv","log_to_file":false}'
[IO.File]::WriteAllText((Join-Path $state 'config.json'), $configuration)
$process = $null
try {
    if (@(Get-ChildItem -LiteralPath $install -File).Count -ne 1) { throw 'Legacy launch requires an executable with no application sidecars.' }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $renamed
    $start.Arguments = '--hidden --state-dir "' + $state + '"'
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.WindowStyle = [Diagnostics.ProcessWindowStyle]::Hidden
    $process = [Diagnostics.Process]::Start($start)
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path -LiteralPath (Join-Path $state 'webview2'))) {
        if ($process.HasExited) { throw "Bare legacy executable exited with code $($process.ExitCode)." }
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Bare legacy executable did not initialize its WebView2 data directory.' }
        Start-Sleep -Milliseconds 50
    }
    Start-Sleep -Seconds 2
    if ($process.HasExited) { throw "Bare legacy executable exited after WebView2 initialization: $($process.ExitCode)." }
    $after = Get-Content -LiteralPath (Join-Path $state 'config.json') -Raw | ConvertFrom-Json
    $before = $configuration | ConvertFrom-Json
    foreach ($property in $before.PSObject.Properties) {
        if (($after.($property.Name) | ConvertTo-Json -Compress -Depth 20) -ne ($property.Value | ConvertTo-Json -Compress -Depth 20)) {
            throw "Legacy launch changed existing setting: $($property.Name)"
        }
    }
    if ((Get-FileHash -LiteralPath $renamed).Hash -ne (Get-FileHash -LiteralPath $source).Hash) { throw 'Renamed legacy executable changed during launch.' }
    Write-Output "Passed: FinFetcher-Legacy.exe renamed to FinFetcher.exe launches and initializes WebView2 without sidecars and preserves settings. Version: $((Get-Item -LiteralPath $renamed).VersionInfo.ProductVersion)"
} finally {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id }
    Write-Output "Legacy launch test files: $root"
}
