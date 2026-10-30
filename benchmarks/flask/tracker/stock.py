"""Unmodified Flask and Werkzeug, CPython sqlite3, and stock Jinja rendering."""
import logging
import os
import sys
from flask import Flask, abort, jsonify, render_template, request, session
from jinja2 import DictLoader
from werkzeug.exceptions import BadRequest
from werkzeug.serving import make_server

import domain
from presentation import DETAIL, LIST

app = Flask("tracker", static_folder=None)
app.secret_key = os.environ["FLASK_SECRET_KEY"]
app.jinja_loader = DictLoader({"list.html": LIST, "detail.html": DETAIL})


def fields():
    value = request.get_json(silent=True)
    if not isinstance(value, dict) or not all(isinstance(x, str) for x in value.values()):
        abort(400)
    return value


@app.errorhandler(ValueError)
def invalid_status(exc):
    return BadRequest()


@app.post("/login")
def login():
    uid = domain.authenticate(fields())
    if not uid:
        return jsonify(error=401), 401
    nonce = domain.new_nonce()
    session.clear()
    session.update(uid=uid, nonce=nonce)
    return jsonify(csrf=str(nonce))


def mutation(result):
    status, ident = result
    return (jsonify(id=ident) if ident else jsonify(error=status)), status


@app.post("/api/issues")
def create():
    return mutation(domain.create(dict(session), fields()))


@app.post("/api/issues/<ident>")
def change(ident):
    return mutation(domain.change(dict(session), ident, fields()))


@app.get("/api/issues")
def issues():
    if not session.get("uid"):
        return jsonify(error=401), 401
    rows = domain.list_issues(session["uid"], request.args.get("status", ""))
    return jsonify(issues=[vars(row) for row in rows])


@app.get("/issues")
def listing():
    if not session.get("uid"):
        return jsonify(error=401), 401
    rows = domain.list_issues(session["uid"], request.args.get("status", ""))
    return render_template("list.html", issues=[domain.context(row) for row in rows])


@app.get("/issues/<ident>")
def detail(ident):
    if not session.get("uid"):
        return jsonify(error=401), 401
    rows = domain.detail(session["uid"], ident)
    if not rows:
        return jsonify(error=404), 404
    return render_template("detail.html", **domain.context(rows[0]))


if __name__ == "__main__":
    logging.getLogger("werkzeug").setLevel(logging.ERROR)
    server = make_server("127.0.0.1", 0, app)
    with open(sys.argv[1], "w") as ready:
        ready.write(str(server.server_port))
    server.serve_forever()
