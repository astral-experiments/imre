"""Run a compiled client against CPython 3.14.5's unchanged ThreadedEchoServer.

With a test-enabled CPython 3.14.5, run this file with a compiled
loopback_client.py executable as its argument, or no argument for the stock control.
The fixture runs separately from upstream unittest accounting.
"""
import json
from pathlib import Path
import subprocess
import sys
from test import support

support.use_resources = {}
support.verbose = False
from test import test_ssl as fixture


def main():
    if sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("this fixture requires test-enabled CPython 3.14.5")
    client = sys.argv[1:] or [sys.executable, "-B", str(Path(__file__).with_name("loopback_client.py"))]
    hostname = fixture.SIGNED_CERTFILE_HOSTNAME
    cases = (
        (False, fixture.SIGNING_CA, hostname, 0),
        (False, fixture.SIGNING_CA, "wrong.example", 62),
        (False, fixture.CAFILE_NEURONIO, hostname, 20),
        (True, fixture.CAFILE_NEURONIO, hostname, 19),
        (False, fixture.SIGNING_CA, "127.0.0.1", 64),
    )
    for chain, cafile, name, expected in cases:
        _, server_context, _ = fixture.testing_context(server_chain=chain)
        server = fixture.ThreadedEchoServer(context=server_context, chatty=False)
        with server:
            argv = [*client, "127.0.0.1", str(server.port), cafile, name, str(expected)]
            child = subprocess.run(argv, capture_output=True, text=True, timeout=30)
        print(json.dumps(dict(server_chain=chain, argv=argv, exit=child.returncode, stdout=child.stdout,
                              stderr=child.stderr, server_errors=server.conn_errors)), flush=True)
        marker = f"VERIFY_ERROR {expected} " if expected else f"VERIFIED_ECHO {hostname}"
        if child.returncode or marker not in child.stdout or bool(server.conn_errors) != bool(expected):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
