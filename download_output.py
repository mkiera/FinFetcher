import os
from pathlib import Path
import shutil
import tempfile


class DownloadOutput:
    def __init__(self, destination):
        self.destination = Path(destination).resolve()
        self.directory = tempfile.mkdtemp(prefix='.finfetcher-', dir=self.destination)
        self.approved = set()
        self.keep = False

    def prepare(self, names, ask, cancelled):
        while not cancelled():
            conflicts = [str(self.destination / name) for name in names
                         if (self.destination / name).exists()
                         and str(self.destination / name) not in self.approved]
            if not conflicts:
                return True
            answer = ask(conflicts)
            if answer['action'] == 'cancel':
                return False
            if answer['action'] == 'overwrite':
                self.approved.update(conflicts)
                return not cancelled()
            self.destination = Path(answer['path']).resolve()
        return False

    def publish(self, cancelled):
        if cancelled():
            return None
        files = [(path, self.destination / path.relative_to(self.directory))
                 for path in Path(self.directory).rglob('*') if path.is_file()]
        if not files:
            raise RuntimeError('The download did not produce a file.')
        conflicts = [str(target) for _, target in files
                     if target.exists() and str(target) not in self.approved]
        if conflicts:
            self.keep = True
            raise RuntimeError('A new filename conflict appeared. The new download is in ' + self.directory)
        if cancelled():
            return None
        published = {}
        for source, target in files:
            target.parent.mkdir(parents=True, exist_ok=True)
            temporary = None
            try:
                handle, temporary = tempfile.mkstemp(prefix='.finfetcher-save-', dir=target.parent)
                os.close(handle)
                shutil.copy2(source, temporary)
                os.replace(temporary, target)
            except OSError:
                self.keep = True
                raise RuntimeError('Could not save the file. The new download is in ' + self.directory)
            finally:
                if temporary and os.path.exists(temporary):
                    os.remove(temporary)
            published[os.path.abspath(source)] = str(target)
        return published

    def cleanup(self):
        if not self.keep:
            shutil.rmtree(self.directory, ignore_errors=True)
