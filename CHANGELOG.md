# Changelog

## Unreleased

## 1.3.0 - 2026-10-30

- Replace the Python desktop application with a Rust and Tauri application while retaining the existing interface and settings.
- Migrate existing installations through the installer with rollback on installation failure.
- Manage yt-dlp as an executable, with verified tool downloads, cancellation during installation and removal of old versions. A yt-dlp copy older than 2025.11.12 is treated as missing, so FinFetcher installs its own copy.
- Try again to install Deno for YouTube downloads a few minutes after a failed attempt, instead of running without it until FinFetcher restarts.
- Stage downloads before replacing existing files, keep the previous files if saving fails, and move finished downloads into place instead of copying them.
- Remove staging folders left by an interrupted or cancelled download, while keeping folders that hold a download FinFetcher could not save.
- Stop media subprocesses and their children before cleaning up cancelled downloads.
- Save to the Downloads folder Windows reports, including a Downloads folder that was moved to another drive or redirected to OneDrive.
- Keep downloading the rest of a playlist when one video fails, then report how many videos failed. A playlist entry that is itself a playlist, such as a channel tab, is explained instead of failing.
- Allow up to 10 minutes to list a large playlist or channel.
- Open FlipperClipper only for single videos, not once for every video in a playlist.
- Show percentage and size for HLS and other fragmented downloads.
- Delete the temporary copies of browser cookies when FinFetcher closes, at the next start after a crash, and on uninstall.
- Keep a damaged settings file as a backup and start with default settings, so settings save and update checks keep working. The log says where the backup is.
- Apply a saved "Log to file" setting from startup, without opening Settings first.
- Keep the latest 1,000 lines in the download log, so long downloads no longer slow the app down.
- Keep a running download's log and preview when a new link is entered, and ignore a slow lookup for an earlier link.
- Order beta releases such as beta.2 and beta.11 by number, and keep prereleases out of the Stable channel.
- Never select a release older than the running version automatically. Its button reads "Downgrade to" and asks for confirmation first.
- Stop the hourly update check when automatic update checks are turned off. Check for Updates still works.
- Remove old update downloads and keep the newest one.

## 1.3.0-beta.3 - 2026-10-30

- Uninstalling always removes the temporary copies of browser cookies, even when you keep your settings.

## 1.3.0-beta.2 - 2026-10-30

- Delete the temporary copy of browser cookies when FinFetcher closes, and remove copies left by earlier sessions at startup.
- Keep downloading the rest of a playlist when one video fails, then report how many videos failed.
- Explain that a playlist entry is itself a playlist, such as a channel tab, instead of reporting a missing file name.
- Allow up to 10 minutes to list a large playlist or channel instead of stopping after 45 seconds.
- Open FlipperClipper only for single videos, not once for every video in a playlist.
- Show percentage and size for HLS and other fragmented downloads instead of "? of Unknown".
- Remove hidden `.finfetcher-stage-` folders left in the save folder by an interrupted or cancelled download, while keeping folders that hold a download FinFetcher could not save.
- Move finished downloads into place instead of copying them, so saving a large video no longer needs two or three times its size in free space.
- Save to the Downloads folder Windows reports, including a Downloads folder that was moved to another drive or redirected to OneDrive.
- A damaged settings file is kept as a backup and replaced with default settings, so settings save and update checks work again. The log says where the backup is.
- Releases older than the running version are no longer selected automatically. Their button reads "Downgrade to" and asks for confirmation first.
- Turning off automatic update checks also stops the hourly check. Check for Updates still works.
- Old yt-dlp versions and old update downloads are removed. The current and previous yt-dlp versions and the newest update download are kept.
- YouTube downloads try again to install Deno a few minutes after a failed attempt, instead of running without it until FinFetcher restarts.
- A yt-dlp copy older than 2025.11.12 is treated as missing, so FinFetcher installs its own copy instead of failing every download.
- A saved "Log to file" setting now applies from startup, without opening Settings first.
- The download log keeps its latest 1,000 lines, so long downloads no longer slow the app down.
- Entering a new link while a download runs no longer clears that download's log and preview, and a slow lookup for an earlier link no longer replaces the current one.
- Fetching a link after an in-app update no longer fails with WinError 448 on PCs whose PATH includes a folder junction.

## 1.3.0-beta.1 - 2026-09-26

- Replace the Python desktop application with a Rust and Tauri application while retaining the existing interface and settings.
- Manage yt-dlp as an executable, with verified tool downloads and cancellation during installation.
- Stop media subprocesses and their children before cleaning up cancelled downloads.
- Stage downloads before replacing existing files, preserving the previous files if saving fails.
- Migrate existing installations through the installer with rollback on installation failure.
- Beta updates recognize numbered releases such as beta.2 and beta.11 in version order.
- Stable updates exclude prereleases even when a release is marked stable on GitHub.
