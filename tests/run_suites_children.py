"""Check that a failed CI suite releases its children without stopping another suite."""

from pathlib import Path
import socket
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_suites import run_suite


CHILD = """\
import socket, sys
with socket.socket(socket.AF_UNIX) as peer:
    peer.settimeout(10)
    peer.connect(sys.argv[1])
    print('ready', flush=True)
    if peer.recv(1) == b'?':
        peer.sendall(b'!')
"""
SUITE = """\
import subprocess, sys
child = subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2]], stdout=subprocess.PIPE)
assert child.stdout.readline() == b'ready\\n'
sys.exit(int(sys.argv[3]))
"""


def main() -> None:
    with tempfile.TemporaryDirectory() as directory, socket.socket(socket.AF_UNIX) as server:
        address = str(Path(directory) / "children.sock")
        server.bind(address)
        server.listen()
        server.settimeout(5)

        def child_after_exit(status: int, cleanup: bool) -> socket.socket:
            command = [sys.executable, "-c", SUITE, CHILD, address, str(status)]
            with tempfile.TemporaryFile(mode="w+") as output:
                assert run_suite(command, output, cleanup) == status
            peer, _ = server.accept()
            peer.settimeout(5)
            return peer

        with child_after_exit(0, True) as healthy, child_after_exit(23, True) as failed:
            assert failed.recv(1) == b""
            healthy.sendall(b"?")
            assert healthy.recv(1) == b"!"
        with child_after_exit(23, False) as baseline:
            baseline.sendall(b"?")
            assert baseline.recv(1) == b"!"
    print("Passed suite exit and child-process controls")


if __name__ == "__main__":
    main()
