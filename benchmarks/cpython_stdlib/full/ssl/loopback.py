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
        (False, False, fixture.SIGNING_CA, hostname, 0),
        (False, False, fixture.SIGNING_CA, "wrong.example", 62),
        (False, False, fixture.CAFILE_NEURONIO, hostname, 20),
        (False, True, fixture.CAFILE_NEURONIO, hostname, 19),
        (False, False, fixture.SIGNING_CA, "127.0.0.1", 64),
        (True, False, fixture.SIGNING_CA, hostname, 0),
    )
    for https_defaults, chain, cafile, name, expected in cases:
        _, server_context, _ = fixture.testing_context(server_chain=chain)
        if https_defaults:
            server_context.set_alpn_protocols(["http/1.1"])
        server = fixture.ThreadedEchoServer(context=server_context, chatty=False)
        with server:
            argv = [*client, "127.0.0.1", str(server.port), cafile, name, str(expected)]
            if https_defaults:
                argv.append("https")
            child = subprocess.run(argv, capture_output=True, text=True, timeout=30)
        print(json.dumps(dict(server_chain=chain, alpn=server.selected_alpn_protocols, argv=argv, exit=child.returncode, stdout=child.stdout,
                              stderr=child.stderr, server_errors=server.conn_errors)), flush=True)
        if https_defaults and server.selected_alpn_protocols != ["http/1.1"]:
            return 1
        marker = f"VERIFY_ERROR {expected} " if expected else f"VERIFIED_ECHO {hostname}"
        if child.returncode or marker not in child.stdout or bool(server.conn_errors) != bool(expected):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
