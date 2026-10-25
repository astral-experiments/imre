"""Call the pinned Python handler synchronously from a native rmtree walk."""
import ast
import builtins
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys


def build_driver(args, folder, source, level):
    glue = Path(__file__).resolve().with_name("rmtree_callback.c")
    obj = folder / "callback.o"
    subprocess.run([args.clang, "-std=c11", "-Wall", "-Wextra", "-Werror", "-c", glue, "-o", obj], check=True)
    entry, binary = folder / "callback.py", folder / "callback"
    entry.write_bytes(source)
    subprocess.run([args.imre, "build", entry, "-o", binary, "--runtime", args.runtime,
                    "--clang", args.clang, "--opt-level", str(level), "--no-cache",
                    "-C", "link-arg=-Wl,--wrap=imre_shutil_rmtree", "-C", f"link-arg={obj}"],
                   check=True, capture_output=True, timeout=180)
    return binary


def native_error(representation, message, fields):
    ctor = ast.parse(representation, mode="eval").body
    assert isinstance(ctor, ast.Call) and isinstance(ctor.func, ast.Name) and not ctor.keywords
    cls = getattr(builtins, ctor.func.id)
    exc = cls(*(ast.literal_eval(arg) for arg in ctor.args))
    if isinstance(exc, OSError):
        errno, strerror, first, second = fields
        if first is not None:
            exc.filename = first
        if second is not None:
            exc.filename2 = second
        assert (exc.errno, exc.strerror, exc.filename, exc.filename2) == (errno, strerror, first, second)
    assert (str(exc), repr(exc)) == (message, representation)
    return exc


def run(binary, argv, dirfds, onerror, onexc, events):
    operations = (os.lstat, os.open, os.path.islink, os.scandir, os.unlink, os.close, os.rmdir)
    parent, child = socket.socketpair()
    with parent, child:
        parent.settimeout(30)
        env = {**os.environ, "IMRE_SHUTIL_FIXTURE_CALLBACK_FD": str(child.fileno())}
        for name, handler in (("ONERROR", onerror), ("ONEXC", onexc)):
            if handler is not None:
                env["IMRE_SHUTIL_FIXTURE_" + name] = "1"
        process = subprocess.Popen([binary, *argv], env=env, pass_fds=(*dirfds, child.fileno()),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        child.close()
        raised = None
        try:
            with parent.makefile("rb") as reader:
                def word():
                    return struct.unpack("=I", reader.read(4))[0]
                def blob():
                    return reader.read(word())
                def field():
                    tag = word()
                    if tag == 0:
                        return None
                    assert tag in (1, 2)
                    value = blob()
                    return value.decode() if tag == 1 else value
                while first := reader.read(4):
                    op = struct.unpack("=I", first)[0]
                    legacy = word()
                    path, representation, message = (blob().decode() for _ in range(3))
                    errno, strerror, filename, filename2 = (field() for _ in range(4))
                    error = native_error(representation, message,
                                         (None if errno is None else int(errno), strerror, filename, filename2))
                    events.append(("onerror" if legacy else "onexc", operations[op].__name__))
                    try:
                        # Reconstructed native error; only the traceback in an
                        # onerror triple is from the original host callback fixture.
                        try:
                            raise error
                        except OSError as caught:
                            if legacy:
                                onerror(operations[op], path, sys.exc_info())
                            else:
                                onexc(operations[op], path, caught)
                    except BaseException as exc:
                        raised = exc
                        parent.sendall(b"R")
                    else:
                        parent.sendall(b"C")
            stdout, stderr = process.communicate(timeout=30)
            if process.returncode or stderr:
                raise RuntimeError(f"native rmtree exited {process.returncode}: {stderr}")
            if raised is not None:
                raise raised
            return stdout.splitlines()
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
