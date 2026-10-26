"""Opt-in Linux evidence for the executable that serves the Flask requests."""
import ast
import hashlib
import http.client
import os
from pathlib import Path
import re
import shutil
import socket
import sys
import time

EXEC = re.compile(r'^\s*(\d+)\s+(execve|execveat)\(')
PROCESS = re.compile(r'^\s*(\d+)\s+(clone3?|fork|vfork)\(')
LIBPYTHON = re.compile(r'/libpython[^/\s<>]*\.so(?:[.\w-]*)\b')
MAPPING = re.compile(r'^\s*\d+\s+mmap2?\(')


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


def inspect_processes(pid, binary):
    todo, seen, sockets = [pid], set(), {}
    while todo:
        current = todo.pop()
        if current in seen:
            continue
        proc = Path(f"/proc/{current}")
        try:
            if not (proc / "exe").samefile(binary):
                raise ValueError(f"unverified executable in the server process tree: {(proc / 'exe').readlink()}")
            if LIBPYTHON.search((proc / "maps").read_text()):
                raise ValueError(f"libpython is mapped in process {current}")
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


def attest_socket(pid, binary, port):
    with socket.create_connection(("127.0.0.1", port), timeout=4) as sock:
        # Leave the headers incomplete until procfs identifies the owner of this
        # accepted connection. A parent that merely publishes a child's port fails.
        sock.sendall(b"GET /health HTTP/1.1\r\nHost: localhost\r\n")
        peer = sock.getsockname()[1]
        loopback = f'{int.from_bytes(socket.inet_aton("127.0.0.1"), sys.byteorder):08X}'
        limit = time.monotonic() + 4
        while time.monotonic() < limit:
            seen, owners = inspect_processes(pid, binary)
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
        if response.status != 200 or response.read() != b'{"ok":true}\n':
            raise ValueError("the verified socket returned the wrong health response")
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
            if LIBPYTHON.search(line):
                raise ValueError(f"mapped libpython in syscall trace: {line}")
    if not images or images[0] != pid:
        raise ValueError("native executable launch evidence is incomplete")
    return {"sha256": hashlib.sha256(raw).hexdigest(), "exec_pids": images,
            "worker_events": process_events, "mmap_count": mappings, "libpython_mappings": 0}
