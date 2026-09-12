import os
import re
import sys

from versioning import read_build_version


def numeric_version(version_string):
    match = re.match(r'^(\d+)(?:\.(\d+))?(?:\.(\d+))?', version_string.strip().lstrip('v'))
    if not match:
        return (0, 0, 0, 0)
    return tuple(int(match.group(i) or 0) for i in (1, 2, 3)) + (0,)


TEMPLATE = """VSVersionInfo(
  ffi=FixedFileInfo(
    filevers={numbers}, prodvers={numbers},
    mask=0x3f, flags=0x0, OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0)),
  kids=[
    StringFileInfo([StringTable('040904B0', [
      StringStruct('CompanyName', 'mkiera'),
      StringStruct('FileDescription', 'FinFetcher - video and music downloader'),
      StringStruct('FileVersion', '{version}'),
      StringStruct('InternalName', 'FinFetcher'),
      StringStruct('LegalCopyright', 'MIT License'),
      StringStruct('OriginalFilename', 'FinFetcher.exe'),
      StringStruct('ProductName', 'FinFetcher'),
      StringStruct('ProductVersion', '{version}')])]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])])
"""


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    version = read_build_version(here, os.environ.get('FINFETCHER_BUILD_VERSION'))

    out_path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, 'version_info.txt')
    with open(out_path, 'w', encoding='utf-8') as f:
        f.write(TEMPLATE.format(numbers=numeric_version(version), version=version))

    print(f'version resource for {version} -> {out_path}')


if __name__ == '__main__':
    main()
