from importlib.machinery import BuiltinImporter

for name, method in (("_opcode", "stack_effect"), ("_bisect", "bisect_right")):
    spec = BuiltinImporter.find_spec(name)
    if spec is not None:
        module = BuiltinImporter.create_module(spec)
        assert vars(module)[method].__self__ is module
        sentinel = object()
        setattr(module, method, sentinel)
        BuiltinImporter.exec_module(module)
        assert getattr(module, method) is sentinel
