"""Register the tracker, check its Jinja programs, and export the native graph."""

from pathlib import Path
import sys
import tempfile

from imre.flask import Flask, export_application, write_renderer

from presentation import DETAIL, LIST

HERE = Path(__file__).resolve().parent
app = Flask("tracker", static_folder=None)
with tempfile.TemporaryDirectory() as tmp:
    renderer = Path(tmp) / "tracker_table.py"
    write_renderer(app.jinja_env, LIST, renderer)
    sys.path.insert(0, tmp)
    import frozen_views as views

    app.post("/login")(views.login)
    app.post("/api/issues", provide_automatic_options=False)(views.create)
    app.post("/api/issues/<ident>")(views.change)
    app.get("/api/issues")(views.issues)
    app.get("/issues")(views.listing)
    app.get("/issues/<ident>")(views.detail)
    export_application(app, {"detail": DETAIL}, sys.argv[1],
                       modules=[HERE / "domain.py", HERE / "storage/tracker_db.py", renderer])
