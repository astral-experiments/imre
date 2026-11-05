import importlib.machinery
import importlib.util
import os
import py_compile
import sys
import tempfile


tag = sys.implementation.cache_tag
extensions = importlib.machinery
extensions.BYTECODE_SUFFIXES.insert(0, ".cached")
extensions.SOURCE_SUFFIXES.insert(0, ".script")
try:
    sys.implementation.cache_tag = "target"
    with tempfile.TemporaryDirectory() as directory:
        source = os.path.join(directory, "plugin.py")
        with open(source, "w", encoding="utf-8") as file:
            file.write("answer = 42\n")

        cached = os.path.join(directory, "__pycache__", "plugin.target.cached")
        assert importlib.util.cache_from_source(source) == cached
        assert importlib.util.source_from_cache(cached) == os.path.join(directory, "plugin.script")
        assert importlib.util.spec_from_file_location("plugin", source).cached == cached

        os.makedirs(cached)
        try:
            py_compile.compile(source, doraise=True)
        except FileExistsError as error:
            assert cached in str(error)
        else:
            raise AssertionError("py_compile accepted a directory as its cache file")
finally:
    sys.implementation.cache_tag = tag
    del extensions.SOURCE_SUFFIXES[0]
    del extensions.BYTECODE_SUFFIXES[0]
