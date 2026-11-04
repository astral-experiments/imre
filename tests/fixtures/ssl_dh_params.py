import os
import ssl
import sys

certdata = sys.argv[1]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
valid = certdata + "/ffdh3072.pem"
assert ctx.load_dh_params(os.fsencode(valid)) is None


class ParamsPath:
    def __init__(self, value):
        self.value = value

    def __fspath__(self):
        events.append("fspath")
        return self.value


def audit(event, args):
    if event == "open" and args[0] is path:
        assert args[1:] == ("rb", 0)
        events.append("open")
        if veto:
            raise PermissionError("audit veto")


events = []
veto = False
path = ParamsPath(valid)
sys.addaudithook(audit)
assert ctx.load_dh_params(path) is None
assert events == ["open", "fspath"]

path = ParamsPath(certdata + "/absent-dh-parameters.pem")
events.clear()
try:
    ctx.load_dh_params(path)
except FileNotFoundError as exc:
    assert exc.filename is path
else:
    raise AssertionError("accepted missing parameters")
assert events == ["open", "fspath"]

events.clear()
veto = True
try:
    ctx.load_dh_params(path)
except PermissionError as exc:
    assert exc.args == ("audit veto",)
else:
    raise AssertionError("ignored audit veto")
assert events == ["open"]

for name in ("keycert.pem", "secp384r1.pem"):
    try:
        ctx.load_dh_params(certdata + "/" + name)
    except ssl.SSLError as exc:
        assert exc.library == "PEM"
        assert exc.reason in ("NO_START_LINE", "UNSUPPORTED_PUBLIC_KEY_TYPE")
        assert exc.reason in str(exc)
    else:
        raise AssertionError("accepted a PEM block without DH parameters")

assert ctx.load_dh_params(valid) is None
