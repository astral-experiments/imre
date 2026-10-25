"""Compare real file transfers, independent descriptor offsets and error arguments."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

SRC = '''import csv
import io
import shutil
import sys
import unittest
from pathlib import Path
src = sys.argv[1]
dst = sys.argv[2]
method = sys.argv[3]
if method == 'flags':
    print(shutil.COPY_BUFSIZE)
    print(shutil._USE_CP_COPY_FILE_RANGE)
    print(shutil._USE_CP_SENDFILE)
    shutil._USE_CP_SENDFILE = False
    shutil._USE_CP_COPY_FILE_RANGE = False
    print(shutil._USE_CP_SENDFILE)
    print(shutil._USE_CP_COPY_FILE_RANGE)
    print(shutil.copyfile(src, dst))
elif method == 'memory':
    with io.BytesIO(b"abc") as a, open(dst, "wb") as d:
        try:
            shutil._fastcopy_sendfile(a, d)
        except csv.Error:
            raise AssertionError('fallback caught by csv.Error')
        except UnicodeEncodeError:
            raise AssertionError('fallback caught by UnicodeEncodeError')
        except shutil.Error:
            raise AssertionError('fallback caught by shutil.Error')
        except OSError:
            raise AssertionError('fallback caught by OSError')
        except shutil._GiveupOnFastCopy as err:
            print(str(err))
            print(repr(err))
elif method == 'assert-raises':
    with io.BytesIO(b'input') as memory, open(dst, 'wb') as dest:
        with unittest.TestCase().assertRaises(shutil._GiveupOnFastCopy) as raised:
            shutil._fastcopy_sendfile(memory, dest)
        print(repr(raised.exception))
    try:
        with unittest.TestCase().assertRaises((shutil._GiveupOnFastCopy, csv.Error)):
            pass
    except AssertionError as error:
        print(str(error))
elif method == 'handlers':
    reader = csv.reader(['"unterminated'], strict=True)
    try:
        next(reader)
    except shutil._GiveupOnFastCopy:
        raise AssertionError('CSV error caught by fast-copy fallback')
    except csv.Error:
        print('csv')
    try:
        shutil.copyfile(src, src)
    except shutil._GiveupOnFastCopy:
        raise AssertionError('same file caught by fast-copy fallback')
    except shutil.SameFileError:
        print('same file')
    try:
        Path('codec').write_text('é', encoding='ascii')
    except shutil._GiveupOnFastCopy:
        raise AssertionError('codec error caught by fast-copy fallback')
    except UnicodeEncodeError:
        print('codec')
    with io.BytesIO(b'input') as memory, open(dst, 'wb') as dest:
        try:
            shutil._fastcopy_copy_file_range(memory, dest)
        except csv.Error:
            raise AssertionError('fallback caught by csv.Error')
        except NameError:
            raise AssertionError('fallback caught by NameError')
        except OSError:
            raise AssertionError('fallback caught by OSError')
        except Exception as err:
            print(repr(err))
elif method == 'same':
    with open(src, "rb") as a:
        try:
            shutil._fastcopy_sendfile(a, a)
        except shutil._GiveupOnFastCopy as err:
            print(str(err))
            print(repr(err))
else:
    with open(src, "rb") as a, open(dst, "wb") as d:
        a.seek(int(sys.argv[4]))
        try:
            if method == 'sendfile':
                shutil._fastcopy_sendfile(a, d)
            else:
                shutil._fastcopy_copy_file_range(a, d)
        except shutil._GiveupOnFastCopy as err:
            print(str(err))
            print(repr(err))
        print(a.tell())
'''
parser = argparse.ArgumentParser(description=__doc__)
for name in ('imre', 'runtime', 'clang'):
    parser.add_argument('--'+name, type=lambda s: Path(s).resolve(), required=True)
args = parser.parse_args()
with tempfile.TemporaryDirectory() as tmp:
    root=Path(tmp)
    script=root/'fast.py'
    script.write_text(SRC)
    def run(cmd):
        results={}
        for method, size, pos in [('flags',31,0),('memory',31,0),('same',31,0),('range',0,0),('sendfile',0,0),('range',113,13),('sendfile',113,13),('handlers',31,0),('assert-raises',31,0)]:
            original=bytes(range(size))
            (root/'source').write_bytes(original)
            (root/'dest').write_bytes(b'target')
            proc=subprocess.run([*cmd,'source','dest',method,str(pos)],cwd=root,capture_output=True)
            assert proc.returncode==0,(method,proc.stderr)
            assert (root/'source').read_bytes()==original
            results[(method,size,pos)]=(proc.stdout,proc.stderr,(root/'dest').read_bytes())
        return results
    oracle=run([sys.executable,'-B',script])
    assert oracle['handlers',31,0][0] == b"csv\nsame file\ncodec\n_GiveupOnFastCopy(UnsupportedOperation('fileno'))\n"
    assert oracle['assert-raises',31,0][0] == (b"_GiveupOnFastCopy(UnsupportedOperation('fileno'))\n"
        b"(<class 'shutil._GiveupOnFastCopy'>, <class '_csv.Error'>) not raised\n")
    for level in ('0','3'):
        program=root/'fast'
        subprocess.run([args.imre,'build',script,'--no-cache','--opt-level',level,'--runtime',args.runtime,'--clang',args.clang,'-o',program],check=True)
        results=run([program])
        for k in oracle:
            assert results[k]==oracle[k],(k,results[k],oracle[k])
        print('O'+level, len(oracle),'transfer, offset, fallback and crossed exception controls matched')
