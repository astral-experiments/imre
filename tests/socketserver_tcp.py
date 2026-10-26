"""Compare one-request, synchronous TCPServer programs with real CPython sockets."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile


FIXTURES = Path(__file__).resolve().parent / "fixtures/socketserver"
REJECT = {
    "timed_listener_subclass": (
        "from socketserver import TCPServer, BaseRequestHandler\n"
        "class Timed(TCPServer):\n"
        "    timeout = 0\n"
        'with Timed(("127.0.0.1", 0), BaseRequestHandler) as server:\n'
        "    server.handle_request()\n",
        "record overrides must have a common result layout",
    ),
    "class_data_is_not_an_instance_field": (
        "class Partial:\n"
        "    shared = True\n"
        "    value: int\n"
        "    def __init__(self) -> None:\n"
        "        inspect(self)\n"
        "def inspect(record: Partial) -> None:\n"
        "    print(record.value)\n"
        "Partial()\n",
        "initialize all instance fields before passing self to a callback",
    ),
    "unsafe_initializer": (
        "from socketserver import BaseRequestHandler, TCPServer\n"
        "import socket\n"
        "class Unsafe(BaseRequestHandler):\n"
        "    flag: bool\n"
        "    def __init__(self, request: socket.socket, address: tuple[str, int], server: TCPServer) -> None:\n"
        "        super().__init__(request, address, server)\n"
        "        self.flag = True\n"
        "    def handle(self) -> None:\n"
        "        print(self.flag)\n",
        "initialize subclass fields before calling a base initializer",
    ),
    "wrong_factory": (
        "from socketserver import BaseRequestHandler, TCPServer\n"
        "from socket import socket\n"
        "class Wrong(BaseRequestHandler):\n"
        "    def __init__(self, request: socket, address: tuple[str, int], server: TCPServer, extra: int) -> None:\n"
        "        super().__init__(request, address, server)\n"
        'TCPServer(("127.0.0.1", 0), Wrong)\n',
        "subclass initializer does not match the factory signature",
    ),
}


TIMED_LISTENER = """from socketserver import TCPServer, BaseRequestHandler
with TCPServer(("127.0.0.1", 0), BaseRequestHandler) as server:
    server.timeout = 0
    try:
        server.handle_request()
        print("timed out")
    except NotImplementedError as error:
        print(str(error))
"""


def observe(command):
    p = subprocess.run(command, capture_output=True, timeout=25)
    if p.returncode or p.stderr:
        raise AssertionError((command, p.returncode, p.stdout, p.stderr))
    return p.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-levels", nargs="+", default=["0", "3"])
    args = parser.parse_args()
    compiler = args.compiler.resolve()
    oracles = {source: observe([args.python, "-E", source]) for source in sorted(FIXTURES.glob("*.py"))}
    with tempfile.TemporaryDirectory(prefix="imre-socketserver-") as tmp:
        root = Path(tmp)
        for name, (source, message) in REJECT.items():
            path = root / (name + ".py")
            path.write_text(source)
            p = subprocess.run([compiler, "check", path], capture_output=True, text=True, timeout=60)
            assert p.returncode == 1 and message in p.stderr, (name, p)
        timed = root / "timed_listener.py"
        timed.write_text(TIMED_LISTENER)
        assert observe([args.python, "-E", timed]) == b"timed out\n"
        for level in args.opt_levels:
            timed_binary = root / "timed_listener"
            subprocess.run([compiler, "build", timed, "--no-cache", "--runtime", args.runtime.resolve(),
                            "--clang", args.clang, "--opt-level", level, "-o", timed_binary], check=True, timeout=180)
            assert observe([timed_binary]) == b"socketserver listener timeout only supports None\n"
            for source, expected in oracles.items():
                binary = root / source.stem
                subprocess.run([compiler, "build", source, "--no-cache", "--runtime", args.runtime.resolve(),
                                "--clang", args.clang, "--opt-level", level, "-o", binary], check=True, timeout=180)
                assert observe([binary]) == expected, source
            print(f"O{level}: TCP handler identity, dispatch, stream data, EOF and descriptor cleanup match CPython")


if __name__ == "__main__":
    main()
