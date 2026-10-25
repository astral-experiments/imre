"""Check actual errors across module families on combined compiler and runtime heads."""
import argparse
import io
from pathlib import Path
import os
import subprocess
import sys
import tarfile
import tempfile

SOURCE = '''import csv
import io
import shutil
import tarfile
from pathlib import Path
def check() -> None:
    try:
        next(csv.reader(['"incomplete'], strict=True))
    except (UnicodeEncodeError, shutil.ReadError, shutil.RegistryError, shutil.Error, tarfile.TarError):
        raise AssertionError("wrong handler for csv.reader")
    except csv.Error:
        print("csv")
    try:
        Path("encoded").write_text("π", encoding="ascii")
    except (csv.Error, shutil.Error, shutil.ReadError, tarfile.TarError):
        raise AssertionError("wrong handler for strict encoding")
    except UnicodeEncodeError:
        print("encoding")
    try:
        shutil.copyfile("file", "file")
    except (csv.Error, UnicodeEncodeError, shutil.ReadError, tarfile.TarError):
        raise AssertionError("wrong handler for same underlying file")
    except shutil.Error:
        print("same file")
    try:
        shutil.copyfile("pipe", "file")
    except (csv.Error, UnicodeEncodeError, shutil.ReadError, shutil.Error, tarfile.SpecialFileError):
        raise AssertionError("wrong handler for a filesystem FIFO")
    except shutil.SpecialFileError:
        print("filesystem FIFO")
    try:
        shutil.unpack_archive("damaged.tar", "dest")
    except (csv.Error, UnicodeEncodeError, shutil.Error, tarfile.TarError):
        raise AssertionError("wrong handler for unreadable tar")
    except shutil.ReadError:
        print("archive")
    try:
        shutil.unpack_archive("damaged.tar", "dest")
    except (csv.Error, UnicodeEncodeError, tarfile.TarError):
        raise AssertionError("wrong base for unreadable tar")
    except OSError:
        print("archive OS base")
    try:
        shutil.unpack_archive("escape.tar", "dest", filter="data")
    except (shutil.Error, shutil.ReadError, csv.Error, UnicodeEncodeError):
        raise AssertionError("wrong handler for outside archive member")
    except tarfile.FilterError:
        print("filter")
    try:
        shutil.unpack_archive("special.tar", "dest", filter="data")
    except (shutil.SpecialFileError, shutil.ReadError, csv.Error, UnicodeEncodeError):
        raise AssertionError("wrong handler for an archive FIFO")
    except tarfile.SpecialFileError:
        print("archive FIFO")
    try:
        raise shutil.RegistryError("format conflict")
    except (OSError, csv.Error, UnicodeEncodeError, tarfile.TarError):
        raise AssertionError("the registry error is a direct Exception subclass")
    except shutil.RegistryError as error:
        print(str(error))
    try:
        raise shutil.RegistryError("format conflict")
    except (shutil.ReadError, shutil.Error, csv.Error):
        raise AssertionError("wrong base for registry errors")
    except Exception:
        print("registry Exception base")
    try:
        raise RuntimeWarning("archive observer")
    except (csv.Error, UnicodeEncodeError, OSError, shutil.RegistryError, tarfile.TarError):
        raise AssertionError("archive family caught RuntimeWarning")
    except Warning:
        print("warning")
    try:
        shutil.unpack_archive("damaged.tar", "dest")
    except Warning:
        raise AssertionError("Warning caught the archive read error")
    except shutil.ReadError:
        print("read error")
    try:
        raise shutil.RegistryError("duplicate")
    except Warning:
        raise AssertionError("Warning caught the registry error")
    except shutil.RegistryError:
        print("registry error")
check()
'''

FASTCOPY = '''
with io.BytesIO(b"source") as memory, open("file", "rb") as disk:
    try:
        shutil._fastcopy_sendfile(memory, disk)
    except (shutil.ReadError, shutil.RegistryError, tarfile.TarError, csv.Error, UnicodeEncodeError, OSError):
        raise AssertionError("unrelated family caught the descriptor fallback")
    except shutil._GiveupOnFastCopy as error:
        print(repr(error))
try:
    shutil.unpack_archive("damaged.tar", "dest")
except shutil._GiveupOnFastCopy:
    raise AssertionError("fast-copy fallback caught an unreadable archive")
except shutil.ReadError:
    print("read error versus fallback")
try:
    shutil.unpack_archive("escape.tar", "dest", filter="data")
except (shutil._GiveupOnFastCopy, shutil.RegistryError):
    raise AssertionError("unrelated family caught the outside-destination error")
except tarfile.OutsideDestinationError:
    print("tar member versus fallback")
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--oracle-only', action='store_true', help='run the crossed handlers with this CPython only')
    parser.add_argument('--with-fastcopy', action='store_true', help='also check the native descriptor fallback on a combined head')
    for arg in ('imre', 'runtime', 'clang'):
        parser.add_argument('--'+arg, type=lambda p: Path(p).resolve())
    args = parser.parse_args()
    if not args.oracle_only and not all((args.imre, args.runtime, args.clang)):
        parser.error('--imre, --runtime and --clang are required for a native comparison')
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source = root/'crossed.py'
        source.write_text(SOURCE + (FASTCOPY if args.with_fastcopy else ''))
        (root/'file').write_bytes(b'keep')
        (root/'dest').mkdir()
        (root/'damaged.tar').write_bytes(b'not a tar file')
        os.mkfifo(root/'pipe')
        for tar, entry, kind in [('escape','../outside',tarfile.REGTYPE),('special','fifo',tarfile.FIFOTYPE)]:
            with tarfile.open(root/(tar+'.tar'), 'w') as archive:
                member = tarfile.TarInfo(entry)
                member.type = kind
                archive.addfile(member, io.BytesIO())
        def run(command):
            completed = subprocess.run(command, cwd=root, capture_output=True, check=True, timeout=30)
            assert not (root/'outside').exists() and (root/'file').read_bytes()==b'keep'
            assert not list((root/'dest').iterdir())
            return completed.stdout, completed.stderr
        expected = run([sys.executable,'-IB',source])
        expected_text = b'csv\nencoding\nsame file\nfilesystem FIFO\narchive\narchive OS base\nfilter\narchive FIFO\nformat conflict\nregistry Exception base\nwarning\nread error\nregistry error\n'
        if args.with_fastcopy:
            expected_text += b"_GiveupOnFastCopy(UnsupportedOperation('fileno'))\nread error versus fallback\ntar member versus fallback\n"
        assert expected == (expected_text, b'')
        count = 16 if args.with_fastcopy else 13
        if args.oracle_only:
            print(f'CPython: {count} crossed handlers matched the expected classes and preserved the fixture')
            return
        for level in ('0','3'):
            binary=root/'check'
            subprocess.run([args.imre,'build',source,'--no-cache','--opt-level',level,'--runtime',args.runtime,
                            '--clang',args.clang,'-o',binary],cwd=root,check=True)
            assert run([binary]) == expected
            print(f'-O{level}: {count} crossed exception handlers matched CPython')

if __name__ == '__main__':
    main()
