"""Diagnostic branch only: show which CPython operations block on this runner."""
import errno
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

for key in ("net.inet.tcp.blackhole", "net.inet.tcp.msl"):
    subprocess.run(["sysctl", key], check=False)
for variant in ("bound", "released", "released-listener", "port-zero"):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        addr = reservation.getsockname()
        if variant == "released-listener":
            reservation.listen()
        if variant.startswith("released"):
            reservation.close()
        if variant == "port-zero":
            addr = ("127.0.0.1", 0)
        with socket.socket() as s:
            s.settimeout(1.5)
            try:
                s.connect(addr)
            except OSError as e:
                print(variant, type(e).__name__, e.errno, str(e), flush=True)
            else:
                print(variant, "connected", flush=True)
src = Path("tests/fixtures/socket/operations.py").read_text()
start = src.index("        # This port stays reserved")
end = src.index("        listener.listen()")
with tempfile.TemporaryDirectory() as tmp:
    path = Path(tmp) / "operations_without_refused.py"
    path.write_text(src[:start] + "\n" * src[start:end].count("\n") + src[end:])
    bootstrap = "import faulthandler,runpy,sys; faulthandler.dump_traceback_later(15,exit=True); runpy.run_path(sys.argv[1],run_name='__main__'); faulthandler.cancel_dump_traceback_later()"
    result = subprocess.run([sys.executable, "-E", "-c", bootstrap, path], timeout=20)
    print("remaining real socket operations exit:", result.returncode, flush=True)
    result.check_returncode()
