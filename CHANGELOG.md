# Changelog

## Unreleased

- Replace the Python desktop application with a Rust and Tauri application while retaining the existing interface and settings.
- Manage yt-dlp as an executable, with verified tool downloads and cancellation during installation.
- Stop media subprocesses and their children before cleaning up cancelled downloads.
- Stage downloads before replacing existing files, preserving the previous files if saving fails.
- Migrate existing installations through the installer with rollback on installation failure.
- Beta updates recognize numbered releases such as beta.2 and beta.11 in version order.
- Stable updates exclude prereleases even when a release is marked stable on GitHub.
- Alpha builds are installed manually.
