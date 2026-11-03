from contextlib import *
from contextlib import __all__ as _exports
import contextlib as _contextlib

_expected = [
    "asynccontextmanager", "contextmanager", "closing", "nullcontext",
    "AbstractContextManager", "AbstractAsyncContextManager", "AsyncExitStack",
    "ContextDecorator", "ExitStack", "redirect_stdout", "redirect_stderr",
    "suppress", "aclosing", "chdir",
]

assert _exports is _contextlib.__all__
assert _exports == _expected
assert {name for name in globals() if not name.startswith("_")} == set(_expected)
for _name in _expected:
    assert globals()[_name] is getattr(_contextlib, _name)
