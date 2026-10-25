"""Compare native AF_INET sockets with CPython, one loopback connection at a time."""
import argparse
from contextlib import contextmanager
from pathlib import Path
import select
import socket
import struct
import subprocess
import sys
import tempfile


FIXTURES = Path(__file__).parent / "fixtures/socket"
TIMEOUT = 15


@contextmanager
def child(command):
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        yield proc
    finally:
        if proc.poll() is None:
            proc.kill()
        proc.wait(timeout=TIMEOUT)


def collect(proc):
    out, err = proc.communicate(timeout=TIMEOUT)
    assert proc.returncode == 0 and err == b"", (proc.returncode, out, err)
    return out


def drain(peer):
    data = bytearray()
    while chunk := peer.recv(65536):
        data.extend(chunk)
    return data


def observe(programs):
    with child(programs["listener"]) as proc:
        assert select.select([proc.stdout], [], [], TIMEOUT)[0], "listener did not report a port"
        port = proc.stdout.readline()
        assert port, collect(proc)
        with socket.create_connection(("127.0.0.1", int(port)), TIMEOUT) as peer:
            peer.sendall(b"\x00a\xff\x80Z")
            peer.shutdown(socket.SHUT_WR)
            echo = drain(peer)
        listener = collect(proc)
    assert echo == b"a\xff\x80Z\x00"
    assert listener == b"True\nb'\\x00'\nb'a\\xff\\x80Z'\nb''\n-1\n-1\n", listener
    with child(programs["operations"]) as proc:
        operations = collect(proc)
    assert operations == b"socket operations passed\n"
    for mode in ("sendall", "reset"):
        with socket.socket() as listener_socket:
            listener_socket.bind(("127.0.0.1", 0))
            listener_socket.listen()
            listener_socket.settimeout(TIMEOUT)
            with child([*programs["client"], mode, str(listener_socket.getsockname()[1])]) as proc:
                peer, _ = listener_socket.accept()
                with peer:
                    peer.settimeout(TIMEOUT)
                    if mode == "reset":
                        assert peer.recv(1) == b"!"
                        peer.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                    else:
                        assert drain(peer) == b"\x00\xffYX" * (1 << 17)
                assert collect(proc) == b"client passed\n"
    return echo, listener, operations


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    args = parser.parse_args()
    sources = {p.stem: p.resolve() for p in FIXTURES.glob("*.py")}
    oracle = observe({name: [args.python, "-E", source] for name, source in sources.items()})
    print("CPython: loopback, fd lifetime, short send, EAGAIN, sendall and reset passed", flush=True)
    with tempfile.TemporaryDirectory(prefix="imre-socket-") as directory:
        for opt in args.opt_levels:
            programs = {}
            for name, source in sources.items():
                binary = Path(directory) / name
                subprocess.run([args.compiler.resolve(), "build", source, "--no-cache", "--runtime",
                                args.runtime.resolve(), "--clang", args.clang, "--opt-level", opt,
                                "-o", binary], check=True, timeout=180)
                programs[name] = [binary]
            assert observe(programs) == oracle
            print(f"O{opt}: socket observations matched CPython", flush=True)


if __name__ == "__main__":
    main()
