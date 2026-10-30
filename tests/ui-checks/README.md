# UI checks

`.github/workflows/ui-check.yml` builds FinFetcher on a GitHub Windows runner, runs every script in
`checks/`, and uploads the screenshots, control lists and logs as the `ui-check-results` artifact.
The runner starts clean on every run, so a check can change settings and the Downloads folder freely.

The workflow downloads a pinned FFmpeg 7.1.1 build and yt-dlp 2025.11.12, checks their SHA-256,
and passes them to the checks as `FF_UI_FFMPEG_DIR` and `FF_UI_YTDLP`.

## Run it

Push a branch. The workflow runs on every push that changes more than Markdown files. To run only
some checks on a branch that is already pushed:

    gh workflow run ui-check.yml --ref <branch> -f filter=<name or wildcard>

`workflow_dispatch` works only once this file is on the default branch. Until then, push.

Read the result:

    gh run list --workflow ui-check.yml --branch <branch> --limit 1
    gh run watch <run id> --exit-status
    gh run download <run id> --name ui-check-results --dir <folder>

Each check gets a folder holding `log.txt`, numbered screenshots, the app's `app-stderr.txt`, any
control lists it wrote and, when it failed, `tree-on-failure.txt` and `on-failure.png` of the whole
screen. `summary.md` lists pass and fail.

## Checks

| Check | Asserts |
| --- | --- |
| `settings-recovery` | A damaged `config.json` is kept as `config.json.corrupt-*`, the notice names it, and saving a setting writes a valid `config.json` again |
| `download` | A video served from 127.0.0.1 lands in the Downloads folder Windows reports (redirected for the check), with `download_log.txt` and no `.finfetcher-stage-*` or `.finfetcher-save-*` folders, and no `cookies-*` folder remains after closing |
| `downgrade-label` | On the Stable channel no older release is selected on its own, and choosing a 1.2.x release labels the button "Downgrade to". The install is never confirmed |

`downgrade-label` reads the GitHub releases API without a token, which allows 60 requests an hour
per address. It fails with a rate-limit message when GitHub refuses. The other checks turn update
checks and yt-dlp updates off so they make no GitHub API calls.

## Write a check

Add `checks/<name>.ps1`. A check passes when it exits without an error.

    $ErrorActionPreference = 'Stop'
    Import-Module $env:FF_UI_MODULE

    $p = Initialize-FfProfile -Settings @{ container = 'mkv' }
    try {
        Start-Ff | Out-Null
        Open-FfSettings
        Find-FfElement -ControlType Button -Name 'Downloads' | Invoke-FfElement
        Save-FfScreenshot -Name 'downloads-settings' | Out-Null
        Assert-Ff ((Get-FfText (Find-FfElement -Id 'containerSelect')) -match 'mkv') 'container is mkv'
    } catch {
        Save-FfTreeOnFailure
        throw
    } finally {
        Stop-Ff | Out-Null
    }

`Initialize-FfProfile` creates an empty data folder for the check and sets `FINFETCHER_DATA_DIR`
to it, so the app never touches `%APPDATA%\FinFetcher`. Its `config.json` points `ffmpeg_path` at
the pinned FFmpeg, turns off update checks and yt-dlp updates, and copies yt-dlp to `<data>\bin`.
`-Settings` overrides keys, `-RawConfig` writes the file text as given, and `-BundledFfmpeg` copies
FFmpeg to `<data>\ffmpeg` for a config without `ffmpeg_path`.

The checks launch windows and the download check changes the user's Downloads folder setting, so
the module and the runner refuse to run outside CI unless `FF_UI_ALLOW_LOCAL=1` is set. Set that only
in a throwaway Windows session.

Artifacts of this public repository can be downloaded. Use only runner paths, 127.0.0.1 addresses
and generated media in a check.

| Function | Does |
| --- | --- |
| `Start-Ff`, `Stop-Ff` | Launch, size the window to 760x1000 and wait for the page. Close the window, kill after 20 seconds, return whether it exited on its own |
| `Find-FfElement -Name -NameLike -Id -ControlType -Index -Scope` | Waits up to 15 seconds for a control. `-Optional` returns nothing instead of failing |
| `Get-FfText` | An element's Name, or the text inside it |
| `Invoke-FfElement`, `Invoke-FfMouseClick` | Clicks through UI Automation, or with the mouse |
| `Switch-FfElement`, `Set-FfText`, `Send-FfKeys` | Toggles a checkbox, fills a text box, sends keys with SendKeys syntax |
| `Open-FfSettings` | Clicks the gear and waits for the Updates section |
| `Wait-Ff { } -Message` | Repeats the block until it returns a value |
| `Get-FfTree -Name` | Writes every named control, with enabled and off-screen state |
| `Save-FfScreenshot -Name` | The app window, captured with `PrintWindow` |
| `Save-FfDesktopScreenshot -Name` | The whole screen, for message boxes |
| `Assert-Ff <condition> <message>` | Fails the check with the message |

WebView2 exposes an HTML `id` as the AutomationId and the visible text as the Name. Elements hidden
with `display: none` are missing from the tree. Run `Get-FfTree` to see what the page exposes.
