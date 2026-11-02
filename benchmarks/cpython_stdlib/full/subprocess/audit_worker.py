"""Bound and reap the processes belonging to one subprocess audit case."""

import ctypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def run_worker(command, cwd, log):
    if sys.platform == "linux":
        # The audit runs cases concurrently. Each case needs its own adopter.
        monitor = subprocess.run([sys.executable, "-B", __file__, *command], cwd=cwd,
                                 stdout=subprocess.PIPE, stderr=log, check=True)
        return json.loads(monitor.stdout)

    child = subprocess.Popen(command, cwd=cwd, stdout=log, stderr=log, start_new_session=True)
    try:
        return child.wait(timeout=65)
    except subprocess.TimeoutExpired:
        return None
    finally:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()


def direct_children():
    parent = os.getpid()
    try:
        yield from map(int, Path(f"/proc/self/task/{parent}/children").read_bytes().split())
        return
    except FileNotFoundError:
        pass  # Some Linux kernels omit the children file.
    for entry in Path("/proc").iterdir():
        if entry.name.isdecimal():
            try:
                stat = (entry / "stat").read_bytes().rsplit(b")", 1)[1].split()
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                continue
            if int(stat[1]) == parent:
                yield int(entry.name)


def reap_descendants():
    deadline = time.monotonic() + 3
    while True:
        try:
            child, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if child:
            continue
        if time.monotonic() >= deadline:
            raise TimeoutError("could not reap all audit worker descendants")
        # Children that changed session also reparent here. A direct child
        # keeps its PID until this single-threaded monitor waits for it.
        for pid in direct_children():
            os.kill(pid, signal.SIGKILL)
        time.sleep(0.01)


def supervise(command):
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER")
    # SIG_IGN can survive exec. Retain waitable children and reserve their PIDs.
    signal.signal(signal.SIGCHLD, signal.SIG_DFL)
    child = subprocess.Popen(command, stdout=sys.stderr, start_new_session=True)
    try:
        return child.wait(timeout=65)
    except subprocess.TimeoutExpired:
        # The timeout leaves this single monitor's session leader unreaped.
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=3)
        return None
    finally:
        reap_descendants()


if __name__ == "__main__":
    print(json.dumps(supervise(sys.argv[1:])))
