"""Opt-in Linux evidence for the executable that serves the Flask requests."""
import ast
import hashlib
import http.client
import os
from pathlib import Path
import re
import shutil
import socket
import stat
import subprocess
import sys
import time

EXEC = re.compile(r'^\s*(\d+)\s+(execve|execveat)\(')
PROCESS = re.compile(r'^\s*(\d+)\s+(clone3?|fork|vfork)\(')
LIBPYTHON = re.compile(r'/libpython[^/\s<>]*\.so(?:[.\w-]*)\b')
MAPPING = re.compile(r'^\s*\d+\s+mmap2?\(')


# strace -y decorates mmap's file descriptor with its backing path.
MAPPED_FD = re.compile(r",\s*(-?\d+)(?:<(.*)>)?,\s*(?:0x[0-9a-fA-F]+|\d+)\)\s+=")


def mapped_python(name, path, cache, identity=None):
    if LIBPYTHON.search(name):
        return True
    if identity is None:
        require_stable_trace_path(path)
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        with os.fdopen(fd, "rb") as stream:
            info = os.fstat(fd)
            if identity is not None and (info.st_dev, info.st_ino) != identity:
                raise ValueError(f"mapped file changed during native proof: {name}")
            if not stat.S_ISREG(info.st_mode):
                return False
            key = (info.st_dev, info.st_ino, info.st_ctime_ns, info.st_size)
            if key not in cache:
                cache[key] = stream.read(4) == b"\x7fELF" and python_elf(fd)
            return cache[key]
    except OSError as error:
        raise ValueError(f"cannot verify mapped file {name}: {error}") from error


def require_stable_trace_path(path):
    """Fail if a post-exit pathname does not have a durable identity."""
    # audit_trace runs after the tracee exits, so it cannot compare the live
    # mapped inode with this pathname. Only trust an immutable system path;
    # otherwise the tracee could replace the file before this open.
    if os.geteuid() == 0:
        raise ValueError(f"cannot establish mapped file identity after exit as root: {path}")
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        try:
            info = current.lstat()
        except OSError as error:
            raise ValueError(f"cannot establish mapped file identity after exit: {path}: {error}") from error
        if stat.S_ISLNK(info.st_mode) or info.st_uid != 0 or info.st_mode & (stat.S_IWGRP | stat.S_IWOTH):
            raise ValueError(f"cannot establish mapped file identity after exit: {path}")


def python_elf(fd):
    reader = shutil.which("readelf") or shutil.which("llvm-readelf")
    if not reader:
        raise ValueError("native proof requires readelf or llvm-readelf")
    # Pass the open descriptor, so renaming a path cannot change the ELF that
    # the reader inspects. The tool reads metadata without loading the image.
    info = subprocess.run([reader, "-W", "-d", "--dyn-syms", f"/proc/self/fd/{fd}"],
                          pass_fds=(fd,), capture_output=True, env={**os.environ, "LC_ALL": "C"})
    if info.returncode or info.stderr:
        raise ValueError(f"cannot verify mapped ELF metadata: {info.stderr.decode(errors='replace')}")
    for soname in re.findall(rb"\(SONAME\)[^\n]*?\[([^\]\n]+)\]", info.stdout):
        if LIBPYTHON.search("/" + os.fsdecode(soname)):
            return True
    # A shared build can omit its SONAME. Undefined imports in an ordinary
    # extension do not identify it as the interpreter.
    missing = {b"Py_Initialize", b"Py_IsInitialized", b"Py_GetVersion"}
    for line in info.stdout.splitlines():
        columns = line.split()
        if len(columns) >= 8 and columns[0].endswith(b":") and columns[6] != b"UND":
            missing.discard(columns[7].split(b"@", 1)[0])
            if not missing:
                return True
    return False


def trace_path(name):
    # strace escapes bytes outside the printable range in fd annotations.
    raw = os.fsencode(name)
    raw = re.sub(rb"\\(?:x([\da-fA-F]{2})|([0-7]{3})|([\\<>]))",
                 lambda m: bytes([int(m[1], 16)]) if m[1] else
                 bytes([int(m[2], 8)]) if m[2] else m[3], raw)
    return os.fsdecode(raw)


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def traced_command(binary, trace):
    if sys.platform != "linux" or not Path("/proc/self/maps").is_file():
        raise ValueError("native proof requires Linux procfs")
    tracer = shutil.which("strace")
    if not tracer:
        raise ValueError("native proof requires strace and permission to trace a child")
    with binary.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise ValueError("the native server must be an ELF executable")
    # -DD leaves the executable at Popen.pid in its own group, so the tracer
    # can record exits while the harness terminates the entire server group.
    # -f follows forked workers and threads; -y names the file backing each mmap.
    return [tracer, "-DD", "-f", "-q", "-z", "-s", "4096", "-y", "-e",
            "trace=execve,execveat,clone,clone3,fork,vfork,mmap,mmap2", "-o", trace, "--", binary]


def require_attached(pid, trace, tracer):
    # A detached strace can leave the program running even when ptrace failed.
    # Verify attachment before the program receives its first HTTP connection.
    status = Path(f"/proc/{pid}/status").read_text().splitlines()
    tracer_pid = int(next(line for line in status if line.startswith("TracerPid:")).split()[1])
    if not tracer_pid or not Path(f"/proc/{tracer_pid}/exe").samefile(tracer):
        raise ValueError("strace did not attach to the native server; check ptrace permissions")
    limit = time.monotonic() + 2
    while time.monotonic() < limit:
        if trace.exists():
            for line in trace.read_text(errors="replace").splitlines():
                if (event := EXEC.match(line)) and int(event[1]) == pid:
                    return tracer_pid
        time.sleep(0.01)
    raise ValueError("strace did not record the native executable launch before serving")


def inspect_processes(pid, binary, image_cache=None):
    image_cache = {} if image_cache is None else image_cache
    todo, seen, sockets = [pid], set(), {}
    while todo:
        current = todo.pop()
        if current in seen:
            continue
        proc = Path(f"/proc/{current}")
        try:
            if not (proc / "exe").samefile(binary):
                raise ValueError(f"unverified executable in the server process tree: {(proc / 'exe').readlink()}")
            for mapping in (proc / "maps").read_text(errors="surrogateescape").splitlines():
                fields = mapping.split(None, 5)
                if len(fields) < 6 or not fields[5].startswith("/"):
                    continue
                name = fields[5].replace(r"\012", "\n")
                dev = os.makedev(*(int(part, 16) for part in fields[3].split(":")))
                if mapped_python(name, proc / "root" / name.lstrip("/"), image_cache,
                                 (dev, int(fields[4]))):
                    raise ValueError(f"libpython is mapped in process {current}: {name}")
            seen.add(current)
            for fd in (proc / "fd").iterdir():
                try:
                    link = os.readlink(fd)
                except FileNotFoundError:
                    continue
                match = re.fullmatch(r"socket:\[(\d+)\]", link)
                if match:
                    sockets.setdefault(match[1], []).append(current)
            for task in (proc / "task").iterdir():
                try:
                    todo.extend(map(int, (task / "children").read_text().split()))
                except FileNotFoundError:
                    pass
        except FileNotFoundError:
            if current == pid:
                raise ValueError("the native server exited during identity verification")
    return seen, sockets


def attest_socket(pid, binary, port, *, route="/health", status=200, body=b'{"ok":true}\n'):
    image_cache = {}
    with socket.create_connection(("127.0.0.1", port), timeout=4) as sock:
        # Leave the headers incomplete until procfs identifies the owner of this
        # accepted connection. A parent that merely publishes a child's port fails.
        sock.sendall(f"GET {route} HTTP/1.1\r\nHost: localhost\r\n".encode("ascii"))
        peer = sock.getsockname()[1]
        loopback = f'{int.from_bytes(socket.inet_aton("127.0.0.1"), sys.byteorder):08X}'
        limit = time.monotonic() + 4
        while time.monotonic() < limit:
            seen, owners = inspect_processes(pid, binary, image_cache)
            inodes = {}
            for line in Path(f"/proc/{pid}/net/tcp").read_text().splitlines()[1:]:
                fields = line.split()
                local, remote = fields[1:3]
                if local != f"{loopback}:{port:04X}" or fields[9] not in owners:
                    continue
                if fields[3] == "0A":
                    inodes["listener"] = fields[9]
                if fields[3] == "01" and remote == f"{loopback}:{peer:04X}":
                    inodes["connection"] = fields[9]
            if len(inodes) == 2:
                break
            time.sleep(0.01)
        else:
            raise ValueError("verified native process tree does not own both the listener and accepted socket")
        sock.sendall(b"Connection: close\r\n\r\n")
        response = http.client.HTTPResponse(sock)
        response.begin()
        if response.status != status or response.read() != body:
            raise ValueError("the verified socket returned the wrong response")
        return {"server_pid": pid, "checked_pids": sorted(seen), "binary_sha256": sha(binary),
                "sockets": {kind: {"inode": inode, "owner_pids": sorted(set(owners[inode]))}
                            for kind, inode in inodes.items()}}


def audit_trace(path, binary, pid):
    # With a detached tracer, wait for its terminal record rather than accepting
    # a partially written log. -q keeps tracee exits while suppressing attaches.
    limit = time.monotonic() + 4
    terminal = re.compile(r"^\s*(\d+)\s+\+\+\+ (?:killed by|exited with)", re.M)
    while time.monotonic() < limit:
        raw = path.read_bytes() if path.exists() else b""
        trace = raw.decode(errors="replace")
        workers = {pid}
        for line in trace.splitlines():
            if PROCESS.match(line):
                if not (result := re.search(r" = (\d+)$", line)):
                    raise ValueError(f"incomplete native worker creation event: {line}")
                workers.add(int(result[1]))
        if workers <= set(map(int, terminal.findall(trace))):
            break
        time.sleep(0.01)
    else:
        raise ValueError("strace failed to record the native server and its workers through exit")
    images, process_events, mappings = [], [], 0
    image_cache = {}
    for line in trace.splitlines():
        if match := EXEC.match(line):
            if match[2] != "execve" or not (arg := re.match(r'("(?:[^"\\]|\\.)*")', line[match.end():])):
                raise ValueError(f"unverified executable transition: {line}")
            try:
                image = Path(ast.literal_eval(arg[1]))
            except (ValueError, SyntaxError) as exc:
                raise ValueError(f"unreadable executable transition: {line}") from exc
            if not image.is_absolute() or not image.is_file() or not image.samefile(binary):
                raise ValueError(f"unverified executable in syscall trace: {image}")
            images.append(int(match[1]))
        elif PROCESS.match(line):
            process_events.append(line)
        elif MAPPING.match(line):
            mappings += 1
            # Anonymous mappings have no file; every file-backed mapping must
            # identify a readable image, including ones unmapped before exit.
            arguments = line.split(",", 4)
            if len(arguments) != 5:
                raise ValueError(f"incomplete native mapping: {line}")
            if "MAP_ANONYMOUS" in arguments[3].strip().split("|"):
                continue
            descriptor = MAPPED_FD.search(line)
            if not descriptor or not descriptor[2]:
                raise ValueError(f"unverified file-backed mapping: {line}")
            name = trace_path(descriptor[2])
            if not name.startswith("/"):
                raise ValueError(f"unverified mapped file path: {name}")
            if mapped_python(name, Path(name), image_cache):
                raise ValueError(f"mapped libpython in syscall trace: {line}")
    if not images or images[0] != pid:
        raise ValueError("native executable launch evidence is incomplete")
    return {"sha256": hashlib.sha256(raw).hexdigest(), "exec_pids": images,
            "worker_events": process_events, "mmap_count": mappings, "libpython_mappings": 0}
