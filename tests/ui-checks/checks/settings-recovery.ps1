# A damaged config.json is moved aside, the app starts on defaults, says where
# the backup is, and saving settings works again.
$ErrorActionPreference = 'Stop'
Import-Module $env:FF_UI_MODULE

$damaged = '{"update_channel": "stable", "log_to_file": '
# The damaged file loses ffmpeg_path, so FFmpeg goes in <data>\ffmpeg to keep the setup screen away.
$p = Initialize-FfProfile -RawConfig $damaged -BundledFfmpeg

try {
    Start-Ff | Out-Null
    Assert-Ff ($null -ne (Find-FfElement -Id 'urlInput' -TimeoutSeconds 5 -Optional)) 'the main view shows, not the FFmpeg setup screen'
    Save-FfScreenshot -Name 'started' | Out-Null

    $backups = @(Get-ChildItem $p.Root -File -Filter 'config.json.corrupt-*')
    Write-Host "backups: $($backups.Name -join ', ')"
    Assert-Ff ($backups.Count -eq 1) 'one config.json.corrupt-* backup exists'
    Assert-Ff ([IO.File]::ReadAllText($backups[0].FullName) -eq $damaged) 'the backup holds the damaged text unchanged'

    if (Test-Path $p.Config) {
        $text = [IO.File]::ReadAllText($p.Config)
        Write-Host "config.json after start: $text"
        Assert-Ff (($text | ConvertFrom-Json -AsHashtable) -is [hashtable]) 'config.json after start is a valid JSON object'
    } else {
        Write-Host 'config.json is absent after start; the app writes it on the first save'
    }

    $stderr = Get-FfStderr
    Write-Host "app stderr: $stderr"
    Write-Host "stderr names the backup: $($stderr -match [regex]::Escape($backups[0].Name))"

    # The notice goes to the download log and to Last Error. The log stays hidden until a download
    # starts, so read it from the debug panel (Ctrl+Shift+D).
    Send-FfKeys '^+d' -Focus (Find-FfElement -Id 'urlInput')
    # debugLastError exposes no AutomationId, so look for the notice text in the window.
    $notice = Wait-Ff -TimeoutSeconds 15 -Message 'the debug panel to show the settings notice' {
        $e = Find-FfElement -NameLike 'config\.json\.corrupt-' -TimeoutSeconds 0 -Optional
        if ($e) { Get-FfText $e }
    }
    Write-Host "Last Error: $notice"
    Assert-Ff ($notice -match [regex]::Escape($backups[0].Name)) 'Last Error names the backup file'
    Assert-Ff ($notice -match 'default settings') 'Last Error says the app uses default settings'
    Save-FfScreenshot -Name 'debug-panel-notice' | Out-Null
    Send-FfKeys '{ESC}'
    Wait-Ff -TimeoutSeconds 5 -Message 'the debug panel to close' {
        -not (Find-FfElement -NameLike 'config\.json\.corrupt-' -TimeoutSeconds 0 -Optional)
    } | Out-Null

    # Saving works again: turn on Log to file in Settings > Downloads.
    Open-FfSettings
    Find-FfElement -ControlType Button -Name 'Downloads' | Invoke-FfElement
    $toggle = Find-FfElement -Id 'logToggle'
    $toggle | Switch-FfElement
    $saved = Wait-Ff -TimeoutSeconds 15 -Message 'config.json to be saved with log_to_file on' {
        if (Test-Path $p.Config) {
            try {
                $c = [IO.File]::ReadAllText($p.Config) | ConvertFrom-Json -AsHashtable
                if ($c -is [hashtable] -and $c['log_to_file'] -eq $true) { $c }
            } catch { $null }
        }
    }
    Write-Host "config.json after save: $($saved | ConvertTo-Json -Compress)"
    Assert-Ff ($saved['log_to_file'] -eq $true) 'config.json is valid and holds the saved setting'
    Save-FfScreenshot -Name 'settings-saved' | Out-Null
    Assert-Ff (@(Get-ChildItem $p.Root -File -Filter 'config.json.corrupt-*').Count -eq 1) 'saving did not create another backup'
} catch {
    Save-FfTreeOnFailure
    throw
} finally {
    Stop-Ff | Out-Null
}
