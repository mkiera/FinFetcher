# Changelog

## Unreleased

- Delete the temporary copy of browser cookies when FinFetcher closes, and remove copies left by earlier sessions at startup.
- Keep downloading the rest of a playlist when one video fails, then report how many videos failed.
- Explain that a playlist entry is itself a playlist, such as a channel tab, instead of reporting a missing file name.
- Allow up to 10 minutes to list a large playlist or channel instead of stopping after 45 seconds.

## 1.3.0-beta.1 - 2026-09-26

- Replace the Python desktop application with a Rust and Tauri application while retaining the existing interface and settings.
- Manage yt-dlp as an executable, with verified tool downloads and cancellation during installation.
- Stop media subprocesses and their children before cleaning up cancelled downloads.
- Stage downloads before replacing existing files, preserving the previous files if saving fails.
- Migrate existing installations through the installer with rollback on installation failure.
- Beta updates recognize numbered releases such as beta.2 and beta.11 in version order.
- Stable updates exclude prereleases even when a release is marked stable on GitHub.
