from inspect import getfullargspec, signature
import sys
from types import ModuleType


class Defaults:
    retries = 3


def send(path, /, *, attempts=1):
    return path


send.__text_signature__ = "(path, /, *, attempts=Defaults.retries)"
assert str(signature(send)) == "(path, /, *, attempts=3)"
Defaults.retries = 7
assert getfullargspec(send).kwonlydefaults == {"attempts": 7}

send.__text_signature__ = "(path, /, *, debug=__debug__)"
print(signature(send).parameters["debug"].default)


class FirstMatch:
    def __init__(self, name):
        self.name = name
        self.comparisons = []

    def __hash__(self):
        return hash(self.name)

    def __eq__(self, name):
        self.comparisons.append(name)
        return len(self.comparisons) == 1


module = ModuleType("clinic_defaults")
sys.modules[module.__name__] = module
send.__module__ = module.__name__
send.__text_signature__ = "(path=clinic_default)"
key = FirstMatch("clinic_default")
module.__dict__[key] = 27
default = signature(send).parameters["path"].default
assert default == 27
assert key.comparisons == ["clinic_default"]

del module.__dict__[key]
del module.__builtins__
module.__dict__[FirstMatch("__builtins__")] = {"clinic_default": 41}
sys.modules["clinic_default"] = 61
default = signature(send).parameters["path"].default
assert default == 61
