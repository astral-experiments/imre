"""Cross-check frozen routing, views and signed sessions against the unchanged wheels."""

import argparse
import http.client
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import zipfile
from http.cookies import SimpleCookie

from flask import Flask
from werkzeug.test import Client
from werkzeug.wrappers import Response
from jinja2 import Environment
from itsdangerous.frozen import FrozenSigner, hmac_sha1
import hashlib
import hmac

parser = argparse.ArgumentParser()
parser.add_argument("catalog", type=Path)
parser.add_argument("wheelhouse", type=Path)
args = parser.parse_args()
from catalog_views import card, stock

secret = "an independent <catalog> secret"
os.environ["FLASK_SECRET_KEY"] = secret
template = "<article>{{ detail }} — {{label}} / {{label}}</article>"
app = Flask("test-catalog", static_folder=None)
app.config.update(SECRET_KEY=secret, SESSION_COOKIE_NAME="catalog")
app.get("/items/<label>")(card)
app.add_url_rule("/items/stock", methods=["PUT", "GET"], view_func=stock)
app.bind_frozen_views({"card": template})
client = Client(app, Response, use_cookies=False)
records = {}


def request(port, method, path, cookie=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
    try:
        conn.request(
            method, path, headers={"Cookie": "catalog=" + cookie} if cookie else {}
        )
        r = conn.getresponse()
        return r.status, dict(r.getheaders()), r.read()
    finally:
        conn.close()


def cookie_of(headers):
    return SimpleCookie(headers["Set-Cookie"])["catalog"].value


# Use the stock wheels in a separate interpreter so no fork code is imported for signing.
with tempfile.TemporaryDirectory() as d:
    temp = Path(d)
    site = temp / "stock"
    site.mkdir()
    for whl in args.wheelhouse.glob("*.whl"):
        with zipfile.ZipFile(whl) as src:
            src.extractall(site)

    def oracle(kind, value, key=secret):
        code = (
            'import json,sys;from flask import Flask;a=Flask("stock");a.secret_key=sys.argv[1];'
            "s=a.session_interface.get_signing_serializer(a);"
            'print(s.dumps(json.loads(sys.argv[3])) if sys.argv[2]=="issue" else json.dumps(s.loads(sys.argv[3])))'
        )
        return subprocess.check_output(
            [sys.executable, "-ESB", "-c", code, key, kind, value], text=True, cwd=site
        ).strip()

    portfile = temp / "port"
    with subprocess.Popen(
        [args.catalog, portfile], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    ) as proc:
        try:
            deadline = time.monotonic() + 8
            while not (portfile.exists() and portfile.read_text().isdigit()):
                assert proc.poll() is None and time.monotonic() < deadline
                time.sleep(0.002)
            port = int(portfile.read_text())
            matrix = [
                ("GET", "/items/%E6%9D%B1%E4%BA%AC%20%3C%26?q=%22one%22&q=two", 200),
                ("GET", "/items/%F0%9F%8C%BF?q=%27", 200),
                ("GET", "/items/stock", 200),
                ("HEAD", "/items/stock", 200),
                ("GET", "/unregistered", 404),
                ("PUT", "/items/other", 405),
                ("POST", "/items/stock", 405),
                ("OPTIONS", "/items/stock", 200),
                ("GET", "/items/one%2Ftwo", 404),
            ]
            for method, path, status in matrix:
                got, headers, body = request(port, method, path)
                wsgi = client.open(path, method=method)
                assert wsgi.status_code == got == status and wsgi.data == body, (
                    method,
                    path,
                    body,
                    wsgi.data,
                )
                assert sorted(wsgi.headers.get("Allow", "").split(", ")) == sorted(
                    headers.get("Allow", "").split(", ")
                ), (method, path)
                records[method + " " + path] = {
                    "status": got,
                    "body": body.decode(),
                    "allow": headers.get("Allow"),
                }
            signed = oracle("issue", json.dumps({"inventory": 1000, "other": 2}))
            code, headers, body = request(port, "PUT", "/items/stock", signed)
            accepted = json.loads(oracle("open", cookie_of(headers)))
            assert (
                code == 200
                and accepted == {"inventory": 997, "other": 2}
                and json.loads(body) == accepted
            )
            records[
                "freshly_signed_by_stock_accepted_by_frozen_and_updated_for_stock"
            ] = accepted
            code, headers, _ = request(
                port,
                "PUT",
                "/items/stock",
                oracle("issue", '{"inventory":1000}', "other-key"),
            )
            assert json.loads(oracle("open", cookie_of(headers))) == {"inventory": 97}
            records["foreign_key_rejected"] = True
            prefix, sig = signed.rsplit(".", 1)
            tampered = prefix + "." + ("A" if sig[0] != "A" else "B") + sig[1:]
            _, headers, _ = request(port, "PUT", "/items/stock", tampered)
            assert json.loads(oracle("open", cookie_of(headers))) == {"inventory": 97}
            records["tampered_signature_rejected"] = True
        finally:
            proc.terminate()
signer = FrozenSigner(b"key", b"cookie-session", 3)
token = signer.sign(b"test", 1700000000)
assert signer.open(token, 1700000003) == b"test"
for when in (1699999999, 1700000004):
    try:
        signer.open(token, when)
    except ValueError:
        pass
    else:
        raise AssertionError("accepted stale or future token")
for key in (b"key", b"K" * 64, b"K" * 111):
    assert hmac_sha1(key, b"Hi There") == hmac.digest(key, b"Hi There", hashlib.sha1)
records["hmac_key_lengths_and_expiry"] = True
env = Environment(autoescape=True)
for text in ("{{ name | upper }}", "{% if name %}Hi{% endif %}"):
    try:
        env.compile_text(text)
    except ValueError:
        pass
    else:
        raise AssertionError(text)
program = env.compile_text("<p>{{x}}{{y}}{{x}}</p>")
context = {"x": "<&>\u2603", "y": "'\x22"}
assert program.render(context) == env.from_string("<p>{{x}}{{y}}{{x}}</p>").render(
    context
)
for rule in ("/<int:x>", "/<path:a>", "/test/"):
    rejected = Flask("rejected", static_folder=None)
    rejected.add_url_rule(rule, view_func=stock)
    try:
        rejected.freeze_routes()
    except ValueError:
        pass
    else:
        raise AssertionError(rule)
records["parser_restrictions_checked"] = True
print(json.dumps(records, indent=2, ensure_ascii=False))
