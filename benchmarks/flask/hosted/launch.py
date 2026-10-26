"""Load the original Imre-compiled app into real CPython Flask and Werkzeug."""
import hashlib
import inspect
import json
from pathlib import Path
import runpy
import sys
from types import ModuleType
from importlib.util import spec_from_file_location

HERE = Path(__file__).resolve().parent
APP = "tiny_flask_app"
sys.path.insert(0, str(HERE))
import _imre_views


def bind_function(code, globals_, name, encoded_signature, defaults):
    spec = json.loads(encoded_signature)
    kinds = (("posonly", inspect.Parameter.POSITIONAL_ONLY),
             ("positional", inspect.Parameter.POSITIONAL_OR_KEYWORD),
             ("kwonly", inspect.Parameter.KEYWORD_ONLY))
    if spec["varargs"] or spec["kwargs"]:
        raise NotImplementedError("variadic views are outside this probe")
    sig = inspect.Signature(
        inspect.Parameter(key, kind, default=defaults.get(key, inspect.Parameter.empty))
        for keys, kind in kinds for key in spec[keys]
    )

    def view(*args, **kwargs):
        bound = sig.bind(*args, **kwargs)
        bound.apply_defaults()
        return _imre_views.invoke(code, name, bound.arguments, globals_, bind_function)

    view.__name__ = view.__qualname__ = name
    view.__module__ = globals_["__name__"]
    view.__signature__ = sig
    view.__imre_llvm__ = code
    return view


def load(site):
    site = Path(site).resolve()
    source = site / (APP + ".py")
    manifest = json.loads((HERE / "build.json").read_text())
    if hashlib.sha256(source.read_bytes()).hexdigest() != manifest["app_sha256"]:
        raise ValueError("app source differs from the compilation input")

    # Refuse CPython execution of any code object from the application itself.
    def forbid_interpretation(event, args):
        if event == "exec" and Path(args[0].co_filename).name == source.name:
            raise RuntimeError("CPython attempted to execute the compiled application")
    sys.addaudithook(forbid_interpretation)
    sys.path.insert(0, str(site))
    module = ModuleType(APP)
    module.__spec__ = spec_from_file_location(APP, source)
    module.__file__ = str(source)
    module.__package__ = ""
    sys.modules[APP] = module
    _imre_views.load_module(APP, vars(module), bind_function)
    app = module.app
    routes = {rule.endpoint for rule in app.url_map.iter_rules()} - {"static"}
    if not routes or any(not hasattr(app.view_functions[key], "__imre_llvm__") for key in routes):
        raise RuntimeError("an application route did not register an LLVM view")
    return app


if __name__ == "__main__":
    site, port = map(Path, sys.argv[1:3])
    load(site)
    entry = site.resolve() / "serve.py"
    sys.argv = [str(entry), str(port)]
    runpy.run_path(str(entry), run_name="__main__")
