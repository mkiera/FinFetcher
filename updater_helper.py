"""
FinFetcher Updater Helper
Standalone script that replaces the old exe with a downloaded update.
Launched by the main app before it exits. Waits for the app to close,
swaps the files, then relaunches the new version.
"""
import argparse
import glob
import os
import sys
import time
import shutil
import subprocess
import tempfile


def cleanup_mei_dirs():
    """Remove leftover PyInstaller _MEI temp directories.
    
    When a --onefile PyInstaller app exits, it may leave behind its
    _MEIxxxxxx extraction directory if another process (like this helper)
    was originally running from it. Clean them up now.
    """
    temp_dir = tempfile.gettempdir()
    for entry in glob.glob(os.path.join(temp_dir, '_MEI*')):
        try:
            if os.path.isdir(entry):
                shutil.rmtree(entry)
        except Exception:
            pass  # Still locked or belongs to another app — skip


def main():
    parser = argparse.ArgumentParser(description='FinFetcher Updater Helper')
    parser.add_argument('--pid', type=int, required=True, help='PID of the running app to wait for')
    parser.add_argument('--old', required=True, help='Path to the current (old) exe')
    parser.add_argument('--new', required=True, help='Path to the downloaded (new) exe')
    args = parser.parse_args()

    # Wait for the main app to exit
    for _ in range(60):  # Max 30 seconds (60 * 0.5s)
        try:
            os.kill(args.pid, 0)  # Check if process is alive (does not kill)
            time.sleep(0.5)
        except OSError:
            break  # Process has exited
    else:
        print("Timed out waiting for app to exit", file=sys.stderr)
        sys.exit(1)

    # Grace period for file handles and _MEI dir to release
    time.sleep(1)

    # Clean up any leftover _MEI temp directories
    cleanup_mei_dirs()

    # Replace the old exe
    try:
        backup_path = args.old + '.bak'

        # Remove old backup if it exists
        if os.path.exists(backup_path):
            os.remove(backup_path)

        # Rename current exe to .bak
        if os.path.exists(args.old):
            os.rename(args.old, backup_path)

        # Move new exe into place
        os.rename(args.new, args.old)

        # Clean up backup
        try:
            os.remove(backup_path)
        except Exception:
            pass  # Not critical — Windows may still lock it briefly

    except Exception as e:
        # Attempt to restore backup
        print(f"Update failed: {e}", file=sys.stderr)
        try:
            if os.path.exists(backup_path) and not os.path.exists(args.old):
                os.rename(backup_path, args.old)
        except Exception:
            pass
        sys.exit(1)

    # Clean up the updates download directory
    try:
        app_data = os.environ.get('APPDATA', os.path.expanduser('~'))
        updates_dir = os.path.join(app_data, 'FinFetcher', 'updates')
        if os.path.isdir(updates_dir):
            shutil.rmtree(updates_dir)
    except Exception:
        pass

    # Relaunch the updated app
    try:
        startupinfo = None
        creationflags = 0
        if os.name == 'nt':
            startupinfo = subprocess.STARTUPINFO()
            startupinfo.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            creationflags = subprocess.CREATE_NO_WINDOW
        subprocess.Popen(
            [args.old],
            startupinfo=startupinfo,
            creationflags=creationflags,
        )
    except Exception as e:
        print(f"Failed to relaunch: {e}", file=sys.stderr)


if __name__ == '__main__':
    main()
