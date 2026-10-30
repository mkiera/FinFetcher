# On the Stable channel an alpha build lists only older releases. None of them is
# selected on its own, and choosing one labels the button "Downgrade to", not "Update to".
# This reads the public GitHub releases of mkiera/FinFetcher without a token. The install is never confirmed.
$ErrorActionPreference = 'Stop'
Import-Module $env:FF_UI_MODULE

$p = Initialize-FfProfile -Settings @{ update_channel = 'stable'; auto_check_updates = $false }

try {
    Start-Ff | Out-Null
    Assert-Ff ($null -ne (Find-FfElement -Id 'urlInput' -TimeoutSeconds 5 -Optional)) 'the main view shows, not the FFmpeg setup screen'
    Open-FfSettings

    $list = Find-FfElement -Id 'releasesList'
    $state = Wait-Ff -TimeoutSeconds 60 -Message 'the Stable release list to load' {
        $message = Find-FfElement -Scope $list -NameLike '^(Error:|Failed to load|No releases found)' -TimeoutSeconds 0 -Optional
        if ($message) { return @{ Message = $message.Current.Name } }
        $rows = @($list.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
            Where-Object { $_.Current.Name -match '^v\d+\.\d+\.\d+' })
        if ($rows.Count) { return @{ Rows = $rows } }
    }
    Get-FfTree -Name 'releases' | Out-Null
    Save-FfScreenshot -Name 'releases-loaded' | Out-Null
    if ($state.ContainsKey('Message')) {
        if ($state.Message -match '403|429|rate limit') {
            throw "GitHub API rate limit: the unauthenticated release list was refused ('$($state.Message)'). Run the workflow again later."
        }
        throw "the release list did not load: '$($state.Message)'"
    }
    $versions = @($state.Rows | ForEach-Object { $_.Current.Name })
    Write-Host "release rows: $($versions -join ', ')"

    # Give a late default selection time to land before reading the button.
    Start-Sleep -Seconds 2
    $button = Find-FfElement -Id 'installSelectedBtn'
    $label = (Get-FfText $button).Trim()
    Write-Host "button before selecting: '$label' enabled=$($button.Current.IsEnabled)"
    Assert-Ff ($label -notmatch '^Update to v1\.2') "the button does not offer 'Update to v1.2...' (reads '$label')"
    Assert-Ff (-not $button.Current.IsEnabled -and $label -eq 'Update') 'no release is selected automatically, so the button is disabled and reads Update'

    $target = $state.Rows | Where-Object { $_.Current.Name -match '^v1\.2\.\d+$' } | Select-Object -First 1
    if (-not $target) {
        Write-Host 'no v1.2.x row is listed, so the Downgrade label could not be checked'
    } else {
        $version = $target.Current.Name
        Write-Host "selecting $version"
        $expected = "Downgrade to $version"
        $target | Invoke-FfMouseClick
        $label = (Get-FfText (Find-FfElement -Id 'installSelectedBtn')).Trim()
        # The row is a div with a click listener. If the mouse missed it, try the element itself and its parents.
        $node = $target
        for ($i = 0; $i -lt 4 -and $label -ne $expected -and $node; $i++) {
            $invoke = Get-FfPattern $node ([System.Windows.Automation.InvokePattern]::Pattern)
            if ($invoke) { $invoke.Invoke(); Start-Sleep -Milliseconds 500 }
            $label = (Get-FfText (Find-FfElement -Id 'installSelectedBtn')).Trim()
            $node = Get-FfParent $node
        }
        Write-Host "button after selecting: '$label'"
        Save-FfScreenshot -Name 'downgrade-selected' | Out-Null
        Assert-Ff ($label -eq $expected) "selecting $version labels the button '$expected' (reads '$label')"
    }
} catch {
    Save-FfTreeOnFailure
    throw
} finally {
    Stop-Ff | Out-Null
}
