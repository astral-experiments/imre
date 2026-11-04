def current():
    import linecache
    import os
    import tempfile

    folder = tempfile.mkdtemp()
    path = os.path.join(folder, "module.py")
    try:
        for data, expected in (
            (b"# coding: latin-1\r\n# caf\xe9\r\nend\rlast", ["# coding: latin-1\n", "# café\n", "end\n", "last\n"]),
            (b"\xef\xbb\xbfstart\xe2\x80\xa8here\n", ["start\u2028here\n"]),
            (b"# one\n# two\nx\x00y\n", ["# one\n", "# two\n", "x\x00y\n"]),
            (b"", ["\n"]),
        ):
            with open(path, "wb") as stream:
                stream.write(data)
            lines = linecache.updatecache(path)
            assert lines == expected
            assert linecache.cache[path][2] is lines is linecache.getlines(path)
            lines[0] = "changed\n"
            assert linecache.getline(path, 1) == "changed\n"
            assert linecache.getline(path, 0) == ""
        for data in (b"# coding: utf-8\n\xff", b"x\x00y\n", b"# first\nx\x00y\n",
                     b"\xef\xbb\xbf# coding: latin-1\n", b"# coding: unknown_imre_codec\n"):
            with open(path, "wb") as stream:
                stream.write(data)
            linecache.checkcache(path)
            assert path not in linecache.cache
            assert linecache.getlines(path) == [] and path not in linecache.cache
        with open(path, "wb") as stream:
            stream.write(b"# coding: rot_13\n")
        try:
            linecache.getlines(path)
        except LookupError:
            pass
        else:
            raise AssertionError("non-text encoding did not raise LookupError")
        assert path not in linecache.cache
        os.unlink(path)
        assert linecache.getlines(path) == []

        class Loader:
            def get_source(self, name):
                assert name == "fallback"
                return "from loader"

        class Spec:
            name = "fallback"
            loader = object()

        globals_with_spec = {"__name__": "fallback", "__loader__": Loader(), "__spec__": Spec()}
        assert not linecache.lazycache(path, globals_with_spec)
        assert linecache.getlines(path, globals_with_spec) == [] and path not in linecache.cache
        assert linecache.lazycache(path, {"__name__": "fallback", "__loader__": Loader()})
        assert linecache.getline(path, 1) == "from loader\n"
        linecache.clearcache()
        assert linecache.cache == {}
    finally:
        if os.path.exists(path):
            os.unlink(path)
        os.rmdir(folder)


    # Frame code, the module spec and the line cache refer to the same provider.
    import __hello__
    import contextlib
    import reprlib
    import zlib
    for module, function in ((contextlib, contextlib.ExitStack.callback),
                             (reprlib, reprlib.recursive_repr)):
        spec = module.__spec__
        source = spec.loader.get_source(spec.name)
        code = function.__code__
        definition = source.splitlines()[code.co_firstlineno - 1]
        assert definition.lstrip().startswith("def " + code.co_name + "(")
        assert spec.origin == code.co_filename
        assert spec.loader.get_code(spec.name).co_filename == spec.origin
        assert linecache.lazycache(code.co_filename, module.__dict__)
        assert linecache.getline(code.co_filename, code.co_firstlineno) == definition + "\n"
        assert linecache.getlines(code.co_filename) is linecache.getlines(code.co_filename)
    assert zlib.__spec__.loader.get_code("zlib") is None
    assert zlib.__spec__.loader.get_source("zlib") is None
    if __hello__.__spec__.origin == "frozen":
        assert __hello__.__spec__.loader.get_code("__hello__").co_name == "<module>"
        assert __hello__.__spec__.loader.get_source("__hello__") is None
    linecache.clearcache()
