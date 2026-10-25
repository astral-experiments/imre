"""Observe no-follow pathname lookups in a single Linux fixture child."""
import ctypes
import os
import platform
import signal
import struct
import tempfile

from rmtree_identity_controls import SYSCALLS, trace


def observe(argv, paths, on_lookup, *, cwd=None):
    """Call on_lookup after the kernel returns and before the child resumes."""
    paths = {os.fsencode(path): path for path in paths}
    max_name = max(map(len, paths)) + 1
    arch = platform.machine()
    _, fstatat, statx = SYSCALLS[arch]
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = os.fork()
        if child == 0:
            try:
                if cwd is not None:
                    os.chdir(cwd)
                os.dup2(stdout.fileno(), 1)
                os.dup2(stderr.fileno(), 2)
                trace(0, 0)  # PTRACE_TRACEME
                os.kill(os.getpid(), signal.SIGSTOP)
                os.execv(argv[0], argv)
            except BaseException as error:
                os.write(2, repr(error).encode())
                os._exit(70)
        live = True
        try:
            _, status = os.waitpid(child, 0)
            if not os.WIFSTOPPED(status):
                live = False
                stderr.seek(0)
                raise RuntimeError(f"child tracing unavailable: {stderr.read()!r}")
            trace(0x4200, child, data=1 | (1 << 20))  # TRACESYSGOOD | EXITKILL
            resume = 0
            action = None
            while True:
                trace(24, child, data=resume)
                _, status = os.waitpid(child, 0)
                if not os.WIFSTOPPED(status):
                    live = False
                    break
                sig = os.WSTOPSIG(status)
                resume = 0 if sig in (signal.SIGTRAP, signal.SIGTRAP | 0x80) else sig
                if sig != signal.SIGTRAP | 0x80:
                    continue
                info = ctypes.create_string_buffer(128)
                trace(0x420e, child, ctypes.sizeof(info), ctypes.addressof(info))
                if info.raw[0] == 1:
                    action = None
                    nr, fd, name, a2, a3, a4 = struct.unpack_from("=6Q", info, 24)
                    if nr == 6 and arch == "x86_64":  # legacy lstat(path, buf)
                        name, buf, syscall = fd, name, "lstat"
                    elif nr in (fstatat, statx) and ctypes.c_int(fd).value == -100:
                        flags, buf = (a2, a4) if nr == statx else (a3, a2)
                        if not flags & 0x100:  # AT_SYMLINK_NOFOLLOW
                            continue
                        syscall = "statx" if nr == statx else "newfstatat"
                    else:
                        continue
                    with open(f"/proc/{child}/mem", "rb", buffering=0) as mem:
                        raw = os.pread(mem.fileno(), max_name, name)
                    raw_name, nul, _ = raw.partition(b"\0")
                    if nul and raw_name in paths:
                        action = paths[raw_name], syscall, buf
                elif info.raw[0] == 2 and action:
                    result, = struct.unpack_from("=q", info, 24)
                    on_lookup(child, *action, result)
                    action = None
        finally:
            if live:
                os.kill(child, signal.SIGKILL)
                os.waitpid(child, 0)
        stdout.seek(0)
        stderr.seek(0)
        output, error = stdout.read().decode(), stderr.read().decode()
        if os.waitstatus_to_exitcode(status) != 0 or error:
            raise RuntimeError(f"fixture exited {status}: {error}")
        return output.splitlines()
