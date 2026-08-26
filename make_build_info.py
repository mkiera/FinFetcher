"""Stamp a build with the commit and CI run that produced it.

version.txt says '1.2.4f-media-options' for every build off that branch, so it
cannot answer "is this the newest alpha?". This writes build_info.json into the
bundle so the Alpha tab can match the running copy against a specific workflow
run instead of guessing from a version string.

Run before PyInstaller; the output is passed with --add-data, so it lands in
sys._MEIPASS next to version.txt where UpdateManager.get_build_info reads it.

Usage: python make_build_info.py [output_path]
"""
import json
import os
import subprocess
import sys
from datetime import datetime, timezone


def _git(*args):
    """A git field, or None when this is not a checkout with git available."""
    try:
        out = subprocess.run(
            ('git',) + args, capture_output=True, text=True, timeout=10,
            cwd=os.path.dirname(os.path.abspath(__file__)) or '.')
    except Exception:
        return None
    value = out.stdout.strip()
    return value if out.returncode == 0 and value else None


def build_info():
    """The identity of this build, from CI's environment or the local repo.

    Every field is optional: a missing one costs an exact match in the UI, so
    it degrades to a timestamp comparison rather than failing the build.
    """
    sha = os.environ.get('GITHUB_SHA') or _git('rev-parse', 'HEAD')
    branch = (os.environ.get('GITHUB_HEAD_REF')
              or os.environ.get('GITHUB_REF_NAME')
              or _git('rev-parse', '--abbrev-ref', 'HEAD'))
    run_id = os.environ.get('GITHUB_RUN_ID')

    # The yt-dlp that got frozen in. Once the app auto-updates yt-dlp into
    # AppData, the bundled version is no longer readable at run time — the
    # managed copy is what imports — so it is recorded here instead.
    try:
        import yt_dlp
        bundled_ytdlp = yt_dlp.version.__version__
    except Exception:
        bundled_ytdlp = None

    return {
        'sha': sha,
        'branch': branch,
        'ytdlp': bundled_ytdlp,
        # Only CI runs have one. A local build leaves it null and is matched by
        # commit, which is right — it was never uploaded as an artifact anyway.
        'run_id': int(run_id) if run_id and run_id.isdigit() else None,
        'built_at': datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
    }


if __name__ == '__main__':
    out_path = sys.argv[1] if len(sys.argv) > 1 else 'build_info.json'
    info = build_info()
    with open(out_path, 'w', encoding='utf-8') as f:
        json.dump(info, f, indent=2)
    print(f'{out_path}: {info}')
