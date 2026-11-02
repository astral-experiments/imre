from importlib import import_module

name = "".join(["__", "future", "__"])
features = import_module(name)
print(features.annotations.compiler_flag)
print(import_module(name) is features)
