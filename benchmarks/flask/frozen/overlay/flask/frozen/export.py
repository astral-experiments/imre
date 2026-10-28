"""Build the static import graph from a registered Flask application."""

from pathlib import Path
from shutil import copyfile
from http import HTTPStatus
import inspect
import keyword

from flask.sessions import FrozenCookieSessionInterface, SecureCookieSessionInterface
from werkzeug.exceptions import default_exceptions


MODULES = (
    "flask/frozen/application.py", "flask/frozen/state.py", "itsdangerous/frozen.py",
    "jinja2/textprogram.py", "markupsafe/_native.py", "werkzeug/routing/frozen.py",
    "werkzeug/wrappers/frozen.py", "werkzeug/frozen_serving.py",
)


def export_application(app, templates, target, *, modules=()):
    """Write the checked app graph into a new directory, excluding dynamic initializers."""
    app.freeze_routes()
    _, bound = app.url_map._frozen
    if (app.before_request_funcs or app.after_request_funcs or app.teardown_request_funcs
            or app.error_handler_spec or app.url_value_preprocessors):
        raise ValueError("frozen deployments require views without request hooks or error overrides")
    if (type(app.session_interface) is not FrozenCookieSessionInterface
            or app.session_interface.key_derivation != "hmac"
            or app.session_interface.digest_method is not SecureCookieSessionInterface.digest_method
            or app.config["SECRET_KEY_FALLBACKS"]
            or app.config["SESSION_COOKIE_DOMAIN"] or app.config["SESSION_COOKIE_SECURE"]
            or app.config["SESSION_COOKIE_PARTITIONED"] or app.config["SESSION_COOKIE_SAMESITE"]
            or not app.config["SESSION_COOKIE_HTTPONLY"] or app.config["SESSION_COOKIE_PATH"] not in (None, "/")
            or app.config["APPLICATION_ROOT"] != "/"):
        raise ValueError("only the standard HttpOnly path=/ session policy is available")
    target = Path(target)
    target.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    for name in MODULES:
        filename = target / name
        filename.parent.mkdir(parents=True, exist_ok=True)
        copyfile(root / name, filename)
        parent = filename.parent
        while parent != target:
            (parent / "__init__.py").touch()
            parent = parent.parent
    code = ["import os", "import sys", "from flask.frozen.application import Application",
            "from flask.frozen.state import AppState", "from werkzeug.wrappers.frozen import Request, Response",
            "from jinja2.textprogram import TextProgram", "from werkzeug.frozen_serving import serve"]
    branches = []
    for n, (rule, capture) in enumerate(bound):
        func = app.view_functions[rule.endpoint]
        if (func.__qualname__ != func.__name__ or "." in func.__module__ or func.__module__ == "__main__"
                or list(inspect.signature(func).parameters) != ["req", "capture", "state"]):
            raise ValueError("frozen views require an importable top-level (req, capture, state) function")
        copyfile(inspect.getfile(func), target / (func.__module__ + ".py"))
        code.append(f"from {func.__module__} import {func.__name__} as view_{n}")
        branches.extend([f"        if index == {n}:", f"            return view_{n}(req, capture, self.state)"])
    for source in map(Path, modules):
        if source.suffix != ".py" or not source.stem.isidentifier() or keyword.iskeyword(source.stem) or source.stem in {"serve", "__init__", "__main__"}:
            raise ValueError(f"expected a top-level application module: {source}")
        if (target / source.name).exists() or (target / source.stem).is_dir():
            raise ValueError(f"duplicate application module: {source.name}")
        copyfile(source, target / source.name)
    code += ["", "class BoundApp(Application):",
             "    def view(self, index: int, req: Request, capture: str) -> Response:", *branches,
             '        raise ValueError("unregistered view")', "", "def main() -> None:",
             '    secret = os.getenv("FLASK_SECRET_KEY", "").encode("utf-8")',
             f"    state = AppState(secret, {app.session_interface.salt.encode()!r}, "
             f"{int(app.permanent_session_lifetime.total_seconds())}, {app.config['SESSION_COOKIE_NAME']!r})",
             "    app = BoundApp(state)"]
    for name, source in templates.items():
        program = app.jinja_env.compile_text(source)
        code.append(f"    state.templates[{name!r}] = TextProgram({program.literals!r}, {program.names!r})")
    methods = set()
    for n, (rule, capture) in enumerate(bound):
        prefix = "".join(value for dynamic, value in rule._trace[1:] if not dynamic)
        for method in sorted(rule.methods):
            methods.add(method)
            code.append(f"    app.rules.add({method!r}, {prefix!r}, {capture!r}, {n})")
        code.append(f"    app.automatic_options.append({rule.provide_automatic_options!r})")
    code.append(f"    app.methods = {sorted(methods)!r}")
    for status in (400, 404, 405, 500):
        body = default_exceptions[status]().get_body().encode()
        code.append(f"    app.errors[{status}] = {body!r}")
    code += [f"    app.reasons[{status.value}] = {status.phrase!r}" for status in HTTPStatus]
    code += ["    serve(app, sys.argv[1])", "", "main()", ""]
    (target / "serve.py").write_text("\n".join(code))
