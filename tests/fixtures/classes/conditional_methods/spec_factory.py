from importlib.machinery import ModuleSpec


def create(name):
    return ModuleSpec(name, None)
