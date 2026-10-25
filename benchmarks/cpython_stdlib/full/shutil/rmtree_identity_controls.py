"""Exchange two real directories after rmtree first inspects the victim's inode."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import struct
import sys
import tempfile

SOURCE = b'''import shutil
try:
    shutil.rmtree('root')
    print('removed')
except OSError as error:
    print(repr(error))
    print(repr(error.filename))
'''

# Linux syscall numbers on the two architectures used by the hosted runs.
SYSCALLS = {'x86_64': (217, 262, 332), 'aarch64': (61, 79, 291)}
LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.ptrace.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p]
LIBC.ptrace.restype = ctypes.c_long


def trace(request, pid, addr=0, data=0):
    if LIBC.ptrace(request, pid, addr, data) == -1:
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))


def run(argv, cwd, unknown, exchange):
    root = cwd / 'root'
    victim = root / 'victim'
    original = cwd / 'original'
    replacement = cwd / 'replacement'
    victim.mkdir(parents=True)
    replacement.mkdir()
    (victim / 'old').write_bytes(b'old inode')
    (replacement / 'keep').write_bytes(b'new inode')
    identity = root.stat()
    getdents, fstatat, statx = SYSCALLS[platform.machine()]
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = os.fork()
        if child == 0:
            try:
                os.chdir(cwd)
                os.dup2(stdout.fileno(), 1)
                os.dup2(stderr.fileno(), 2)
                trace(0, 0)  # TRACEME: only the forked fixture is traced.
                os.kill(os.getpid(), signal.SIGSTOP)
                os.execv(argv[0], argv)
            except BaseException as error:
                os.write(2, repr(error).encode())
                os._exit(70)
        stamped = looked_up = swapped = 0
        live = True
        try:
            _, status = os.waitpid(child, 0)
            if not os.WIFSTOPPED(status):
                live = False
                stderr.seek(0)
                raise RuntimeError(f'fixture cannot trace its child: {stderr.read()!r}')
            trace(0x4200, child, data=1 | (1 << 20))  # TRACESYSGOOD | EXITKILL
            resume_signal = 0
            action = None
            while True:
                trace(24, child, data=resume_signal)  # PTRACE_SYSCALL
                _, status = os.waitpid(child, 0)
                if not os.WIFSTOPPED(status):
                    live = False
                    break
                sig = os.WSTOPSIG(status)
                resume_signal = 0 if sig in (signal.SIGTRAP, signal.SIGTRAP | 0x80) else sig
                if sig != signal.SIGTRAP | 0x80:
                    continue
                info = ctypes.create_string_buffer(128)
                trace(0x420e, child, ctypes.sizeof(info), ctypes.addressof(info))  # GET_SYSCALL_INFO
                if info.raw[0] == 1:  # syscall entry
                    action = None
                    number, fd, path_or_buf = struct.unpack_from('=3Q', info, 24)
                    if number not in (getdents, fstatat, statx) or fd > 0x7fffffff:
                        continue
                    try:
                        same_dir = os.path.samestat(identity, os.stat(f'/proc/{child}/fd/{fd}'))
                    except FileNotFoundError:
                        continue
                    if not same_dir:
                        continue
                    if number == getdents:
                        action = ('entries', path_or_buf)
                    else:
                        with open(f'/proc/{child}/mem', 'rb', buffering=0) as mem:
                            if os.pread(mem.fileno(), 7, path_or_buf) == b'victim\x00':
                                action = ('stat', 0)
                elif info.raw[0] == 2 and action:  # syscall exit; kernel result is still live
                    result, = struct.unpack_from('=q', info, 24)
                    op, address = action
                    if op == 'entries' and result > 0:
                        with open(f'/proc/{child}/mem', 'rb+', buffering=0) as mem:
                            entries = os.pread(mem.fileno(), result, address)
                            at = 0
                            while at < len(entries):
                                size, = struct.unpack_from('=H', entries, at + 16)
                                assert size >= 20 and at + size <= len(entries)
                                name = entries[at+19:at+size].split(b'\x00', 1)[0]
                                if name == b'victim':
                                    stamped += 1
                                    if unknown:
                                        # Preserve the real kernel entry, but withhold its optional d_type.
                                        assert os.pwrite(mem.fileno(), b'\x00', address + at + 18) == 1
                                at += size
                    elif op == 'stat' and result == 0:
                        looked_up += 1
                        if exchange and not swapped:
                            victim.rename(original)
                            replacement.rename(victim)
                            swapped += 1
                    action = None
        finally:
            if live:
                os.kill(child, signal.SIGKILL)
                os.waitpid(child, 0)
        stdout.seek(0)
        stderr.seek(0)
        text, error = stdout.read().decode(), stderr.read().decode()
        assert os.waitstatus_to_exitcode(status) == 0 and not error, (status, text, error)
    assert stamped == 1 and looked_up > 0 and swapped == int(exchange), (stamped, looked_up, swapped)
    return {'output': text.splitlines(), 'victim_stats': looked_up,
            'old_preserved': (original / 'old').is_file(), 'replacement_preserved': (victim / 'keep').is_file(),
            'root_exists': root.exists()}


def compare(tools, directory):
    source = directory / 'case.py'
    source.write_bytes(SOURCE)
    binary = tools.build(SOURCE, directory) if tools else None
    cases = {}
    for unknown in (False, True):
        for exchange in (False, True):
            label = f'{"unknown" if unknown else "known"}-{"exchanged" if exchange else "stable"}'
            expected = run([sys.executable, '-B', str(source)], directory / (label+'-reference'), unknown, exchange)
            assert expected['root_exists'] == expected['replacement_preserved'] == expected['old_preserved'] == exchange
            row = {'reference': expected}
            if binary:
                actual = run([str(binary)], directory / (label+'-native'), unknown, exchange)
                row.update(native=actual, matches=actual == expected)
            cases[label] = row
    return cases


if __name__ == '__main__':
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from suite_native import Toolchain

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + name, type=lambda p: Path(p).resolve(), required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform != 'linux' or platform.machine() not in SYSCALLS:
        parser.error('requires pinned CPython 3.14.5 on Linux x86-64 or aarch64')
    result = {'schema': 'imre.shutil.directory-identity/v1', 'platform': platform.machine(),
              'scope': 'supplemental real filesystem controls; no original test credit', 'optimization_levels': {}}
    for name in ('imre', 'runtime'):
        with getattr(args, name).open('rb') as stream:
            result[name + '_sha256'] = hashlib.file_digest(stream, 'sha256').hexdigest()
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            result['optimization_levels'][level] = compare(tools, Path(tmp))
    args.result.write_text(json.dumps(result, indent=2) + '\n')
    bad = [(level, name) for level, cases in result['optimization_levels'].items()
           for name, row in cases.items() if not row['matches']]
    print('mismatches:', bad)
    sys.exit(bool(bad))
