"""Differential controls for unsafe tar entries and metadata; these add no upstream IDs."""
import argparse
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile


PROGRAM = '''import shutil
import sys
import tarfile
try:
    shutil.unpack_archive(sys.argv[1], 'dest', filter='data')
    print('success')
except shutil.ReadError as e:
    print('shutil.ReadError')
    print(str(e))
except tarfile.OutsideDestinationError as e:
    print('outside file')
    print(str(e))
except tarfile.AbsoluteLinkError as e:
    print('absolute link')
    print(str(e))
except tarfile.LinkOutsideDestinationError as e:
    print('outside link')
    print(str(e))
except tarfile.SpecialFileError as e:
    print('special file')
    print(str(e))
'''


def fixtures(root):
    specs = [('parent', '../outside/marker', tarfile.REGTYPE, ''),
             ('absolute', 'new/absolute', tarfile.SYMTYPE, '/escape'),
             ('relative', 'new/relative', tarfile.SYMTYPE, '../../outside/marker'),
             ('hardlink', 'new/hard', tarfile.LNKTYPE, '../outside/marker'),
             ('fifo', 'new/pipe', tarfile.FIFOTYPE, ''),
             ('existing_link', 'trap/marker', tarfile.REGTYPE, '')]
    for key, name, kind, link in specs:
        with tarfile.open(root/f'{key}.tar', 'w') as archive:
            member = tarfile.TarInfo(name)
            member.type, member.linkname = kind, link
            archive.addfile(member)
    with tarfile.open(root/'metadata.tar', 'w') as archive:
        for name, kind in [('tree', tarfile.DIRTYPE), ('tree/data', tarfile.REGTYPE)]:
            member = tarfile.TarInfo(name)
            member.type, member.mode = kind, 0o6067
            member.mtime = 1_750_000_000.125
            member.size = 4 if kind == tarfile.REGTYPE else 0
            archive.addfile(member, io.BytesIO(b'data'))
    (root/'invalid.tar').write_bytes(b'no tar header')
    return [item[0] for item in specs] + ['metadata', 'invalid']


def run(root, command, key):
    dest = root/'dest'
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir()
    (dest/'trap').symlink_to('../outside')
    result = subprocess.run([*command, f'{key}.tar'], cwd=root, capture_output=True, check=True, timeout=30)
    assert not result.stderr and (root/'outside/marker').read_bytes() == b'keep'
    assert not (dest/'new').exists(), 'a rejected member modified the destination'
    stat = []
    if key == 'metadata':
        for p in (dest/'tree', dest/'tree/data'):
            info = os.stat(p)
            stat.append((info.st_mode, info.st_mtime_ns))
        assert (dest/'tree/data').read_bytes() == b'data'
    return result.stdout, stat


def zstd_interop(args, root, level):
    """The pinned interpreter lacks compression.zstd; use independent zstd frames."""
    zstd = shutil.which('zstd')
    if not zstd:
        print(f'-O{level}: independent zstd interoperability control unavailable: no zstd executable')
        return
    src = root/'zstd.py'
    src.write_text('''import shutil
import sys
if sys.argv[1] == "write":
    print(shutil.make_archive("written", "zstdtar", "source"))
else:
    shutil.unpack_archive("independent.tar.zst", "independent")
''')
    binary = root/'zstd-archive'
    subprocess.run([args.imre, 'build', src, '--no-cache', '--opt-level', level, '--runtime', args.runtime,
                    '--clang', args.clang, '-o', binary], check=True, cwd=root)
    source = root/'source'
    source.mkdir(exist_ok=True)
    (source/'data').write_bytes(b'interoperable\x00archive')
    result = subprocess.run([binary, 'write'], cwd=root, check=True, capture_output=True)
    assert Path(os.fsdecode(result.stdout.strip())).name == 'written.tar.zst'
    data = subprocess.run([zstd, '-dc', root/'written.tar.zst'], check=True, capture_output=True).stdout
    with tarfile.open(fileobj=io.BytesIO(data)) as made:
        assert made.extractfile('./data').read() == (source/'data').read_bytes()
    with tarfile.open(root/'independent.tar', 'w') as other:
        other.add(source/'data', arcname='nested/file')
    subprocess.run([zstd, '-q', '-f', root/'independent.tar', '-o', root/'independent.tar.zst'], check=True)
    subprocess.run([binary, 'read'], cwd=root, check=True)
    assert (root/'independent/nested/file').read_bytes() == (source/'data').read_bytes()
    print(f'-O{level}: native zstdtar write/read interoperated with system zstd and tarfile')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang'):
        parser.add_argument('--'+name, type=lambda p: Path(p).resolve(), required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root/'outside').mkdir()
        (root/'outside/marker').write_bytes(b'keep')
        src = root/'check.py'
        src.write_text(PROGRAM)
        keys = fixtures(root)
        expected = {key: run(root, [sys.executable, '-IB', src], key) for key in keys}
        for level in ('0', '3'):
            binary = root/'check'
            subprocess.run([args.imre, 'build', src, '--no-cache', '--opt-level', level, '--runtime', args.runtime,
                            '--clang', args.clang, '-o', binary], check=True, cwd=root)
            for key in keys:
                actual = run(root, [binary], key)
                assert actual == expected[key], (level, key, actual, expected[key])
            print(f'-O{level}: {len(keys)} independent tar filters, diagnostics and metadata controls matched CPython')
            zstd_interop(args, root, level)


if __name__ == '__main__':
    main()
