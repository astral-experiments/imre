"""Run the unchanged Linux zero-copy classes using real files and their original syscall mocks."""
import argparse
import array
import ast
from contextlib import chdir, ExitStack
import errno
import io
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_in_fixtures

DRIVER = '''import shutil
import sys
import io
from typing import BinaryIO
def zero(src: BinaryIO, dst: BinaryIO) -> None:
    if sys.argv[2] == "sendfile":
        shutil._fastcopy_sendfile(src, dst)
    else:
        shutil._fastcopy_copy_file_range(src, dst)
def run() -> None:
    shutil._USE_CP_COPY_FILE_RANGE = sys.argv[3] == "1"
    shutil._USE_CP_SENDFILE = sys.argv[4] == "1"
    suppressed = False
    try:
        if sys.argv[1] == "public":
            shutil.copyfile(sys.argv[5], sys.argv[6])
        elif sys.argv[1] == "memory-src":
            with io.BytesIO() as a, open("/imre-shutil-fixture/destination-fd","wb") as b:
                zero(a,b)
        elif sys.argv[1] == "memory-dst":
            with open("/imre-shutil-fixture/source-fd","rb") as c, io.BytesIO() as d:
                zero(c,d)
        else:
            with open("/imre-shutil-fixture/source-fd","rb") as e, open("/imre-shutil-fixture/destination-fd","wb") as f:
                zero(e,f)
        print("ok")
        print("None")
    except shutil._GiveupOnFastCopy as err:
        print("Giveup")
        print(repr(err))
        suppressed = err.__suppress_context__
    except FileNotFoundError as err:
        print("FileNotFoundError")
        print(repr(err))
    except OSError as err:
        print("OSError")
        print(repr(err))
        suppressed = err.__suppress_context__
    print(shutil._USE_CP_SENDFILE)
    print(suppressed)
run()
'''

def shell(exe, argv, env, fds, original_os, summary):
    """Only the original test's mocks may supply a result. All unpatched calls run in the child."""
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    frames = []
    with parent, child:
        parent.settimeout(60)
        env["SHUTIL_FAST_SOCKET"] = str(child.fileno())
        with subprocess.Popen([exe, *argv], env=env, pass_fds=(*fds, child.fileno()),
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as proc:
            child.close()
            try:
                while True:
                    body, anc, flags, _ = parent.recvmsg(24, socket.CMSG_SPACE(8))
                    if not body:
                        break
                    assert not flags and len(body) == 24
                    op, off, size = struct.unpack('=qqq', body)
                    rights = array.array('i')
                    assert len(anc) == 1 and anc[0][:2] == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
                    rights.frombytes(anc[0][2])
                    try:
                        action = value = 0
                        if op == 3:
                            summary["fallback_reads"] += 1
                        else:
                            name = ("copy_file_range", "sendfile", "fstat")[op]
                            callback = getattr(original_os, name)
                            frame = {"syscall":name, "offset":off, "size":size, "mock":isinstance(callback, Mock)}
                            frames.append(frame)
                            if isinstance(callback, Mock):
                                summary["mock_callbacks"] += 1
                                try:
                                    r = (callback(rights[0], rights[1], size, offset_dst=off) if op == 0
                                         else callback(rights[1], rights[0], off, size) if op == 1
                                         else callback(rights[0]).st_size)
                                    action, value = 1, int(r)
                                except OSError as exc:
                                    # fstat's size fallback ignores errno altogether. The kernel
                                    # boundary transports POSIX errno, not a Python strerror override.
                                    if exc.errno is None and op != 2:
                                        raise
                                    action, value = 2, exc.errno or errno.EIO
                                    frame["errno"] = exc.errno
                        parent.sendall(struct.pack('=qq', action, value))
                    finally:
                        for fd in rights:
                            os.close(fd)
                out, err = proc.communicate(timeout=60)
                assert proc.returncode == 0 and not err, (proc.returncode, out, err)
            except BaseException:
                proc.kill()
                raise
    summary["syscalls"].extend(frames)
    return out.splitlines()


def run_level(main, tools, work, shim):
    exe = tools.build(DRIVER.encode(), work)
    def execute(case, original):
        name = case._testMethodName
        if name in ("test_unhandled_exception", "test_blocksize_arg"):
            return {"status":"blocked", "reason":"original os syscall mock raises ZeroDivisionError; a POSIX errno cannot carry an arbitrary Python exception through the native copyfile loop"}
        if name in ("test_non_existent_src", "test_exception_on_second_call"):
            field = 'filename' if name == 'test_non_existent_src' else 'errno'
            return {"status":"blocked", "reason":f"original handler reads OSError.{field}; this head cannot transport that native field without the shared structured OS error projection"}
        summary = {"native_operations":[], "syscalls":[], "mock_callbacks":0, "fallback_reads":0,
                   "host_stream_copy":0,"flag_changes":0}
        lib = main.shutil
        if isinstance(case, main.TestZeroCopySendfile):
            method = 'sendfile'
        else:
            method = 'range'
        def invoke(mode, srcpath, dstpath, rfd=None, wfd=None):
            summary["native_operations"].append("copyfile" if mode=="public" else "_fastcopy_"+method)
            env = dict(os.environ, LD_PRELOAD=str(shim))
            if srcpath is not None:
                env["SHUTIL_FAST_SOURCE"] = os.fspath(srcpath)
            if rfd is not None:
                env["SHUTIL_FIXTURE_SRC"] = str(rfd)
            if wfd is not None:
                env["SHUTIL_FIXTURE_DST"] = str(wfd)
            if isinstance(main.os.fstat, Mock):
                env['SHUTIL_FAST_FSTAT'] = '1'
            if isinstance(lib.copyfileobj,Mock) and mode=='public':
                env['SHUTIL_FAST_GUARD_READ'] = '1'
            argv=[mode,method,str(int(lib._USE_CP_COPY_FILE_RANGE)),str(int(lib._USE_CP_SENDFILE)),srcpath or '',dstpath or '']
            out=shell(exe,argv,env,tuple({f for f in (rfd,wfd) if f is not None}),main.os,summary)
            assert len(out)==4,out
            status, payload, sendfile, suppressed = out
            next_flag = {"True":True,"False":False}[sendfile]
            if lib._USE_CP_SENDFILE != next_flag:
                lib._USE_CP_SENDFILE = next_flag
                summary["flag_changes"] += 1
            if 'SHUTIL_FAST_GUARD_READ' in env:
                assert summary['fallback_reads']==0, 'native copyfile read the source despite the upstream copyfileobj mock'
            if status=='ok':
                return dstpath if mode=='public' else None
            parsed=ast.parse(payload, mode='eval').body
            assert isinstance(parsed,ast.Call) and isinstance(parsed.func,ast.Name)
            if status=='Giveup':
                assert parsed.func.id=='_GiveupOnFastCopy'
                if not parsed.args:
                    raise main._GiveupOnFastCopy()
                wrapped,=parsed.args
                assert isinstance(wrapped,ast.Call) and isinstance(wrapped.func,ast.Name)
                cls={'OSError':OSError,'UnsupportedOperation':io.UnsupportedOperation,'ValueError':ValueError}[wrapped.func.id]
                raise main._GiveupOnFastCopy(cls(*(ast.literal_eval(a) for a in wrapped.args)))
            # Preserve arguments originating in the native exception, never input paths.
            cls = {'FileNotFoundError':FileNotFoundError, 'OSError':OSError, 'PermissionError':PermissionError,
                   'IsADirectoryError':IsADirectoryError}[parsed.func.id]
            raised=cls(*(ast.literal_eval(a) for a in parsed.args))
            raised.__suppress_context__={'True':True,'False':False}[suppressed]
            if raised.errno==errno.ENOSPC:
                assert raised.__suppress_context__
                summary['ENOSPC_from_None']=True
            raise raised
        def direct(src,dst):
            a=isinstance(src,io.BytesIO)
            b=isinstance(dst,io.BytesIO)
            return invoke('memory-src' if a else 'memory-dst' if b else 'private',
                          None if a else src.name,None if b else dst.name,
                          None if a else src.fileno(),None if b else dst.fileno())
        def public(src,dst):
            return invoke('public',src,dst)
        stream_copy=lib.copyfileobj
        def copy_stream(src,dst,*args,**kwargs):
            summary['host_stream_copy']+=1
            return stream_copy(src,dst,*args,**kwargs)
        previous=lib._USE_CP_SENDFILE
        try:
            with ExitStack() as stack:
                stack.enter_context(patch.object(lib,"_fastcopy_sendfile" if method=="sendfile" else "_fastcopy_copy_file_range",direct))
                stack.enter_context(patch.object(lib,"copyfile",public))
                if name.startswith('test_non_regular_file_'):
                    stack.enter_context(patch.object(lib,'copyfileobj',copy_stream))
                original()
        finally:
            lib._USE_CP_SENDFILE=previous
        assert summary['native_operations']
        if name=='test_regular_copy':
            assert summary['native_operations']==['_fastcopy_'+method,'copyfile']
            assert any(f['syscall']==('sendfile' if method=='sendfile' else 'copy_file_range') for f in summary['syscalls'])
        mixed = summary['host_stream_copy'] or summary['mock_callbacks']
        return {"status":"mixed" if mixed else "host_driven_native", **summary}
    suite=load_suite('test.test_shutil')
    selected={c.id():execute for c in cases(suite) if isinstance(c,(main.TestZeroCopySendfile,main.TestZeroCopyCopyFileRange))}
    good,fail,skip=run_in_fixtures(suite,selected)
    rows={n:v[0] for n,v in good.items() if v}
    rows.update({n:{"status":"failed","reason":reason} for n,reason in fail})
    rows.update({n:{"status":"reference_skip","reason":reason} for n,reason in skip})
    assert rows.keys()==selected.keys() and len(rows)==29
    return rows


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for arg in ('imre','runtime','clang','result'):
        parser.add_argument('--'+arg,type=lambda p:Path(p).resolve(),required=True)
    args=parser.parse_args()
    if sys.version_info[:3]!=(3,14,5) or sys.platform!='linux':
        parser.error('requires test-enabled Linux CPython 3.14.5')
    manifest=json.loads((HERE/'UPSTREAM.json').read_text())
    verify_files(STDLIB/'upstream',manifest['files'])
    verify_interpreter_tests(manifest)
    import test
    tests=STDLIB/'upstream/Lib/test'
    test.__path__.insert(0,str(tests))
    levels={}
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        root=Path(tmp)
        from test import test_shutil
        assert Path(test_shutil.__file__).resolve()==tests/'test_shutil.py'
        shim=root/'syscalls.so'
        subprocess.run([args.clang,'-shared','-fPIC','-Wall','-Werror',HERE/'fastcopy_mock.c',HERE/'stream_fds.c','-o',shim],check=True)
        for level in (0,3):
            work=root/str(level)
            work.mkdir()
            levels[level]=run_level(test_shutil,Toolchain(args.imre,args.runtime,args.clang,level),work,shim)
    args.result.write_text(json.dumps({'cpython_commit':manifest['commit'],'optimization_levels':levels},indent=2)+'\n')
    return int(any(row['status']=='failed' for rows in levels.values() for row in rows.values()))

if __name__=='__main__':
    sys.exit(main())
