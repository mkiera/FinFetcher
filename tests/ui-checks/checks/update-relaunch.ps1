# An in-app update ends with Setup starting FinFetcher again. That copy must not run under
# RedirectionGuard: yt-dlp inherits it and then fails with WinError 448 on junctions in PATH.
# The check installs the built FinFetcher-Setup.exe, runs it again with the updater's switches
# while the app is open, and downloads a generated video in the copy Setup started.
$ErrorActionPreference = 'Stop'
Import-Module $env:FF_UI_MODULE

$installer = $env:FF_UI_INSTALLER
if (-not $installer -or -not (Test-Path $installer)) { throw "FF_UI_INSTALLER does not point at FinFetcher-Setup.exe: '$installer'" }
$p = Initialize-FfProfile
$out = $env:FF_UI_OUT
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\FinFetcher'
$work = Join-Path ([IO.Path]::GetTempPath()) 'finfetcher-ui-update'
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null

function Invoke-Setup([string[]]$Switches, [string]$Name) {
    $log = Join-Path $out "$Name.log"
    $setup = Start-Process -FilePath $installer -ArgumentList ($Switches + "/LOG=`"$log`"") -PassThru
    if (-not $setup.WaitForExit(180000)) { throw "$Name Setup did not finish within 3 minutes" }
    Assert-Ff ($setup.ExitCode -eq 0) "$Name Setup exited with code 0 (got $($setup.ExitCode))"
    $line = Select-String -LiteralPath $log -Pattern 'RedirectionGuard status' | Select-Object -Last 1
    Write-Host "$Name Setup log: $(if ($line) { $line.Line.Trim() } else { 'no RedirectionGuard line' })"
}

function Wait-Started([int[]]$Except = @()) {
    Wait-Ff -TimeoutSeconds 90 -Message 'Setup to start FinFetcher' {
        Get-Process FinFetcher -ErrorAction SilentlyContinue |
            Where-Object { $_.Id -notin $Except -and $_.Path -like "$installDir\*" } | Select-Object -First 1
    }
}

$probe = $null
$server = $null
$saved = $null
try {
    # Control: a process that turns the guard on must read as guarded, or the checks below prove nothing.
    $probeScript = @'
Add-Type 'using System; using System.Runtime.InteropServices; public static class G { [DllImport("kernel32.dll")] public static extern bool SetProcessMitigationPolicy(int p, ref uint v, int s); }'
$v = [uint32]1
[void][G]::SetProcessMitigationPolicy(16, [ref]$v, 4)
Start-Sleep -Seconds 120
'@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($probeScript))
    $probe = Start-Process pwsh -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile', '-EncodedCommand', $encoded
    $control = Wait-Ff -TimeoutSeconds 30 -Message 'the control process to turn RedirectionGuard on' { Get-FfRedirectionGuard $probe }
    Assert-Ff $control 'the probe reads RedirectionGuard as on in a process that enabled it'

    Invoke-Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/SP-', '/NORESTART') 'install'
    $first = Wait-Started
    Connect-Ff -Process $first | Out-Null
    Assert-Ff (-not (Get-FfRedirectionGuard $first)) 'the copy Setup starts after installing runs without RedirectionGuard'

    # The same switches the in-app updater passes, with the app still open.
    Invoke-Setup @('/SILENT', '/SP-', '/NOCANCEL', '/NORESTART', '/CLOSEAPPLICATIONS', '/NORESTARTAPPLICATIONS') 'update'
    Assert-Ff ($first.HasExited) 'the update closed the running copy'
    $second = Wait-Started -Except @($first.Id)
    Connect-Ff -Process $second | Out-Null
    Assert-Ff (-not (Get-FfRedirectionGuard $second)) 'the copy Setup starts after the update runs without RedirectionGuard'
    Save-FfScreenshot -Name 'after-update' | Out-Null

    $ffmpeg = Join-Path $env:FF_UI_FFMPEG_DIR 'ffmpeg.exe'
    & $ffmpeg -hide_banner -loglevel error -f lavfi -i 'testsrc=duration=2:size=320x240:rate=25' `
        -f lavfi -i 'sine=frequency=440:duration=2' -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest `
        (Join-Path $work 'finfetcher-ui-update.mp4')
    if ($LASTEXITCODE -ne 0) { throw "ffmpeg could not generate the test video (exit $LASTEXITCODE)" }
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
    $url = "http://127.0.0.1:$port/finfetcher-ui-update.mp4"
    $python = (Get-Command python -ErrorAction Stop).Source
    $server = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList '-m', 'http.server', "$port", '--bind', '127.0.0.1', '--directory', "`"$work`""
    Wait-Ff -TimeoutSeconds 20 -Message "the test server at $url" {
        try { (Invoke-WebRequest -Uri $url -Method Head -TimeoutSec 2).StatusCode -eq 200 } catch { $false }
    } | Out-Null

    $saved = Join-Path (Get-FfKnownDownloads) 'finfetcher-ui-update.mp4'
    if (Test-Path $saved) { Remove-Item -Force $saved }
    Find-FfElement -Id 'urlInput' | Set-FfText -Value $url
    Find-FfElement -Id 'downloadBtn' | Invoke-FfElement
    $outcome = Wait-Ff -TimeoutSeconds 240 -Message 'the download in the updated copy to finish' {
        $done = Find-FfElement -NameLike 'Download Complete|Download failed|Download error|Download cancelled' -TimeoutSeconds 0 -Optional
        if ($done) { $done.Current.Name }
    }
    Save-FfScreenshot -Name 'after-download' | Out-Null
    Assert-Ff ($outcome -match 'Download Complete') "the updated copy completed a download (log says '$outcome')"
    Assert-Ff ((Test-Path $saved) -and (Get-Item $saved).Length -gt 0) 'the video was saved in Downloads'
} catch {
    Save-FfTreeOnFailure
    throw
} finally {
    Stop-Ff | Out-Null
    if ($probe -and -not $probe.HasExited) { Stop-Process -Id $probe.Id -Force }
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force }
    if ($saved -and (Test-Path $saved)) { Remove-Item -Force $saved }
    $uninstaller = Join-Path $installDir 'unins000.exe'
    if (Test-Path $uninstaller) {
        $u = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -PassThru
        $u.WaitForExit(60000) | Out-Null
    }
}
