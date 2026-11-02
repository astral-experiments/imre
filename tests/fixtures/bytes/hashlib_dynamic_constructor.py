from sys import modules

name = "".join(("hash", "lib"))
module = __import__(name)
assert modules[name] is module
module.__builtin_constructor_cache["sha256"] = False
assert module.__get_builtin_constructor("sha256") is False
