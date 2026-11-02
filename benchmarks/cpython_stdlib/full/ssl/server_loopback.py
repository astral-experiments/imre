"""Run with test-enabled CPython 3.14.5 and a compiled loopback_server.py.

Omit the executable for the stock control. These loopback checks run separately
from upstream unittest accounting.
"""
import json
from pathlib import Path
import select
import socket
import ssl
import subprocess
import sys
from test import support

support.use_resources = {}
support.verbose = False
from test import test_ssl as fixture


def exchange(server, version, cafile, hostname, expected):
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    ctx.minimum_version = ctx.maximum_version = version
    ctx.load_verify_locations(cafile)
    argv = [*server, "reject" if expected else "serve", fixture.SIGNED_CERTFILE, "-"]
    with subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True) as child:
        line = ""
        observed = {}
        try:
            assert select.select([child.stdout], [], [], 10)[0], "server did not announce port"
            line = child.stdout.readline()
            assert line.startswith("PORT "), line
            with socket.create_connection(("127.0.0.1", int(line.split()[1])), timeout=10) as raw:
                try:
                    peer = ctx.wrap_socket(raw, server_hostname=hostname)
                except ssl.SSLCertVerificationError as exc:
                    observed["verify_code"] = exc.verify_code
                    assert expected and exc.verify_code == expected
                else:
                    with peer:
                        assert not expected
                        binding = peer.get_channel_binding()
                        assert binding
                        observed["binding"] = binding.hex()
                        observed["protocol"] = peer.version()
                        peer.sendall(b"LOCAL TLS\n")
                        with peer.makefile("rb") as reply:
                            assert reply.readline(11) == b"local tls\n"
        finally:
            try:
                tail, errors = child.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                tail, errors = child.communicate()
            print(json.dumps(dict(argv=argv, hostname=hostname, cafile=cafile, version=version.name,
                                  observed=observed, exit=child.returncode, stdout=line + tail,
                                  stderr=errors)), flush=True)
    assert child.returncode == 0
    if expected:
        assert any(line.startswith("PEER_REJECTED ") for line in tail.splitlines())
    else:
        assert "BINDING " + observed["binding"] in tail.splitlines()


def main():
    if sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("this fixture requires test-enabled CPython 3.14.5")
    server = sys.argv[1:] or [sys.executable, "-B", str(Path(__file__).with_name("loopback_server.py"))]
    for mode, certfile, keyfile in (("load", fixture.ONLYCERT, fixture.ONLYKEY),
                                    ("invalid", fixture.BADCERT, "-"),
                                    ("invalid", fixture.ONLYCERT, fixture.BADKEY),
                                    ("invalid", fixture.CAFILE_CACERT, fixture.ONLYKEY)):
        argv = [*server, mode, certfile, keyfile]
        p = subprocess.run(argv, capture_output=True, stdin=subprocess.DEVNULL, text=True, timeout=10)
        print(json.dumps(dict(argv=argv, exit=p.returncode, stdout=p.stdout, stderr=p.stderr)), flush=True)
        assert p.returncode == 0
        assert p.stdout.startswith("LOAD_ERROR" if mode == "invalid" else "LOADED")

    host = fixture.SIGNED_CERTFILE_HOSTNAME
    for version in (ssl.TLSVersion.TLSv1_2, ssl.TLSVersion.TLSv1_3):
        exchange(server, version, fixture.SIGNING_CA, host, 0)
    exchange(server, ssl.TLSVersion.TLSv1_3, fixture.SIGNING_CA, "wrong.example", 62)
    exchange(server, ssl.TLSVersion.TLSv1_2, fixture.CAFILE_NEURONIO, host, 20)


if __name__ == "__main__":
    main()
