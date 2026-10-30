# A download saves to the Downloads folder Windows reports,
# writes download_log.txt from the saved setting without opening Settings, leaves no staging
# folders, and the exported browser cookies are gone after the app closes.
# The video is generated here and served from 127.0.0.1, so no outside site or personal data is used.
$ErrorActionPreference = 'Stop'
Import-Module $env:FF_UI_MODULE

$p = Initialize-FfProfile -Settings @{ log_to_file = $true }
$work = Join-Path ([IO.Path]::GetTempPath()) 'finfetcher-ui-download'
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
$media = Join-Path $work 'media'
$redirected = Join-Path $work 'Redirected Downloads'
New-Item -ItemType Directory -Force -Path $media, $redirected | Out-Null
$out = $env:FF_UI_OUT

$ffmpeg = Join-Path $env:FF_UI_FFMPEG_DIR 'ffmpeg.exe'
& $ffmpeg -hide_banner -loglevel error -f lavfi -i 'testsrc=duration=2:size=320x240:rate=25' `
    -f lavfi -i 'sine=frequency=440:duration=2' -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest `
    (Join-Path $media 'finfetcher-ui-test.mp4')
if ($LASTEXITCODE -ne 0) { throw "ffmpeg could not generate the test video (exit $LASTEXITCODE)" }

$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
$listener.Start()
$port = $listener.LocalEndpoint.Port
$listener.Stop()
$url = "http://127.0.0.1:$port/finfetcher-ui-test.mp4"

$shellFolders = 'Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders'
$downloadsValue = '{374DE290-123F-4565-9164-39C4925E467B}'
$homeDownloads = Join-Path $env:USERPROFILE 'Downloads'
$server = $null
$key = $null
$oldValue = $null
$oldKind = $null

try {
    $python = (Get-Command python -ErrorAction Stop).Source
    $server = Start-Process -FilePath $python -PassThru -WindowStyle Hidden `
        -ArgumentList '-m', 'http.server', "$port", '--bind', '127.0.0.1', '--directory', "`"$media`"" `
        -RedirectStandardOutput (Join-Path $out 'http-server-stdout.txt') `
        -RedirectStandardError (Join-Path $out 'http-server-stderr.txt')
    Wait-Ff -TimeoutSeconds 20 -Message "the test server at $url" {
        try { (Invoke-WebRequest -Uri $url -Method Head -TimeoutSec 2).StatusCode -eq 200 } catch { $false }
    } | Out-Null
    Write-Host "serving $url"

    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($shellFolders, $true)
    $oldValue = $key.GetValue($downloadsValue, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if ($null -ne $oldValue) { $oldKind = $key.GetValueKind($downloadsValue) }
    Write-Host "Downloads was '$oldValue', now '$redirected'"
    $key.SetValue($downloadsValue, $redirected, [Microsoft.Win32.RegistryValueKind]::ExpandString)

    # A new process reads the known folder the way the app does, which separates a redirect that
    # did not take from an app that ignores it.
    $reported = & pwsh -NoProfile -Command "Import-Module '$env:FF_UI_MODULE'; Get-FfKnownDownloads"
    Write-Host "SHGetKnownFolderPath in a new process: $reported"
    Assert-Ff ($reported -eq $redirected) 'Windows reports the redirected Downloads folder to new processes'

    $started = Get-Date
    $homeBefore = @(if (Test-Path $homeDownloads) { Get-ChildItem $homeDownloads -Force -Recurse -ErrorAction SilentlyContinue | ForEach-Object FullName })

    Start-Ff | Out-Null
    Assert-Ff ($null -ne (Find-FfElement -Id 'urlInput' -TimeoutSeconds 5 -Optional)) 'the main view shows, not the FFmpeg setup screen'
    Save-FfScreenshot -Name 'before-download' | Out-Null

    Find-FfElement -Id 'urlInput' | Set-FfText -Value $url
    Find-FfElement -Id 'downloadBtn' | Invoke-FfElement

    $cookieDirsSeen = [System.Collections.Generic.HashSet[string]]::new()
    $outcome = Wait-Ff -TimeoutSeconds 240 -IntervalMilliseconds 250 -Message 'the download to finish' {
        Get-ChildItem $p.Root -Directory -Filter 'cookies-*' -ErrorAction SilentlyContinue | ForEach-Object { $cookieDirsSeen.Add($_.Name) | Out-Null }
        $done = Find-FfElement -NameLike 'Download Complete|Download failed|Download error|Download cancelled' -TimeoutSeconds 0 -Optional
        if ($done) { $done.Current.Name }
    }
    Write-Host "outcome: $outcome"
    Write-Host "cookie export folders seen during the run: $(if ($cookieDirsSeen.Count) { $cookieDirsSeen -join ', ' } else { 'none' })"
    Get-FfTree -Name 'after-download' | Out-Null
    Save-FfScreenshot -Name 'after-download' | Out-Null
    Assert-Ff ($outcome -match 'Download Complete') "the download completed (log says '$outcome')"

    $saved = @(Get-ChildItem $redirected -File -Filter '*.mp4')
    Write-Host "redirected folder: $((Get-ChildItem $redirected -Force | ForEach-Object Name) -join ', ')"
    Assert-Ff ($saved.Count -eq 1 -and $saved[0].Length -gt 0) 'the video was saved in the redirected Downloads folder'

    $homeNew = @(if (Test-Path $homeDownloads) {
            Get-ChildItem $homeDownloads -Force -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notin $homeBefore -or $_.LastWriteTime -ge $started } | ForEach-Object FullName
        })
    Write-Host "new in $homeDownloads`: $(if ($homeNew.Count) { $homeNew -join ', ' } else { 'nothing' })"
    Assert-Ff ($homeNew.Count -eq 0) 'nothing was written to %USERPROFILE%\Downloads'

    $logFile = Join-Path $redirected 'download_log.txt'
    Assert-Ff (Test-Path $logFile) 'download_log.txt was written next to the video without opening Settings'
    $logText = [IO.File]::ReadAllText($logFile)
    Copy-Item $logFile (Join-Path $out 'download_log.txt')
    Assert-Ff ($logText -match 'Preparing download') 'download_log.txt holds the download log'

    $leftovers = @(Get-ChildItem $redirected -Force | Where-Object { $_.Name -like '.finfetcher-stage-*' -or $_.Name -like '.finfetcher-save-*' })
    Assert-Ff ($leftovers.Count -eq 0) "no .finfetcher-stage-* or .finfetcher-save-* left in the folder ($($leftovers.Name -join ', '))"

    $beforeClose = @(Get-ChildItem $p.Root -Directory -Filter 'cookies-*' | ForEach-Object Name)
    Write-Host "cookie export folders before closing: $(if ($beforeClose.Count) { $beforeClose -join ', ' } else { 'none' })"
    Assert-Ff (Stop-Ff) 'FinFetcher closed normally when its window was closed'
    $afterClose = @(Get-ChildItem $p.Root -Directory -Filter 'cookies-*' | ForEach-Object Name)
    Assert-Ff ($afterClose.Count -eq 0) "no cookies-* folder remains in the data folder after closing ($($afterClose -join ', '))"
} catch {
    Save-FfTreeOnFailure
    throw
} finally {
    Stop-Ff | Out-Null
    if ($key) {
        if ($null -eq $oldValue) { $key.DeleteValue($downloadsValue, $false) }
        else { $key.SetValue($downloadsValue, $oldValue, $oldKind) }
        $key.Dispose()
        Write-Host 'Downloads folder setting restored'
    }
    if ($server -and -not $server.HasExited) { Stop-Process -Id $server.Id -Force }
}
