import os
import shutil
import subprocess


def _registered_flipperclipper():
    try:
        import winreg

        key_path = (
            r'Software\Microsoft\Windows\CurrentVersion\Uninstall'
            r'\{A67FDBB5-CF45-489D-8A41-0E7576A446F1}_is1'
        )
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, key_path) as key:
            install_dir, _ = winreg.QueryValueEx(key, 'InstallLocation')
    except (ImportError, OSError):
        return None

    if not isinstance(install_dir, str) or not install_dir:
        return None
    executable = os.path.join(install_dir, 'FlipperClipper.exe')
    return executable if os.path.isfile(executable) else None


def find_flipperclipper():
    if os.name != 'nt':
        return None

    registered = _registered_flipperclipper()
    if registered:
        return os.path.abspath(registered)

    for name in ('FlipperClipper.exe', 'FlipperClipper'):
        found = shutil.which(name)
        if found and os.path.isfile(found):
            return os.path.abspath(found)

    roots = (
        os.environ.get('LOCALAPPDATA'),
        os.environ.get('ProgramFiles'),
        os.environ.get('ProgramFiles(x86)'),
    )
    relative_paths = (
        os.path.join('Programs', 'FlipperClipper', 'FlipperClipper.exe'),
        os.path.join('FlipperClipper', 'FlipperClipper.exe'),
    )
    for root in filter(None, roots):
        for relative_path in relative_paths:
            candidate = os.path.join(root, relative_path)
            if os.path.isfile(candidate):
                return os.path.abspath(candidate)
    return None


def open_in_flipperclipper(video_path, executable=None):
    executable = executable or find_flipperclipper()
    if not executable:
        return False, 'FlipperClipper is no longer installed.'

    if not isinstance(video_path, str) or not video_path:
        return False, 'The download finished without a video path.'
    video_path = os.path.abspath(video_path)
    if not os.path.isfile(video_path):
        return False, f'The downloaded video was not found: {video_path}'

    creationflags = 0
    if os.name == 'nt':
        creationflags = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
    try:
        subprocess.Popen(
            [executable, video_path],
            close_fds=True,
            creationflags=creationflags,
        )
    except OSError as error:
        return False, f'FlipperClipper did not open: {error}'
    return True, None
