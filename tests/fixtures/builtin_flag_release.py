from importlib.machinery import BuiltinImporter

spec = BuiltinImporter.find_spec("_opcode")
if spec is not None:
    module = BuiltinImporter.create_module(spec)
    seen = []

    class Displaced:
        def __del__(self):
            seen.append(hasattr(module, "ENABLE_SPECIALIZATION_FT"))

    module.ENABLE_SPECIALIZATION = Displaced()
    BuiltinImporter.exec_module(module)
    assert seen == [False]
