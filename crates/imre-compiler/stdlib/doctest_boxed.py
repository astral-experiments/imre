# The boxed importer discovers docs from the live module instead of a fixed module graph.
from collections import namedtuple
import os
import sys
import types


# From CPython 3.14.5, Lib/doctest.py (PSF-2.0).
class TestResults(namedtuple('TestResults', 'failed attempted')):
    def __new__(cls, failed, attempted, *, skipped=0):
        results = super().__new__(cls, failed, attempted)
        results.skipped = skipped
        return results

    def __repr__(self):
        if self.skipped:
            return (f'TestResults(failed={self.failed}, '
                    f'attempted={self.attempted}, '
                    f'skipped={self.skipped})')
        else:
            # Leave the repr() unchanged for backward compatibility
            # if skipped is zero
            return super().__repr__()


def _from_module(item, owner):
    name = getattr(item, "__module__", None)
    module = sys.modules.get(name)
    if module is not None:
        return module is owner
    if isinstance(item, types.FunctionType):
        return item.__globals__ is owner.__dict__
    return name == owner.__name__


def _boxed_docs(obj, name, owner, found, seen):
    key = id(obj)
    if key in seen:
        return
    seen.add(key)
    doc = obj if isinstance(obj, str) else getattr(obj, "__doc__", None)
    if isinstance(doc, str) and doc:
        found[name] = (obj, doc)
    is_module = isinstance(obj, types.ModuleType)
    if not is_module and not isinstance(obj, type):
        return
    routines = (types.FunctionType, types.BuiltinFunctionType, types.MethodType)
    for attr, item in obj.__dict__.items():
        if not is_module and isinstance(item, (staticmethod, classmethod)):
            item = item.__func__
        if not is_module and isinstance(item, property):
            _boxed_docs(item, name + "." + attr, owner, found, seen)
        elif isinstance(item, type) or isinstance(item, routines):
            if _from_module(item, owner):
                _boxed_docs(item, name + "." + attr, owner, found, seen)
    if is_module:
        for attr, item in getattr(obj, "__test__", {}).items():
            if not isinstance(attr, str):
                raise ValueError("DocTestFinder.find: __test__ keys must be strings: %r" % (type(attr),))
            if not isinstance(item, (str, types.ModuleType, type)) and not isinstance(item, routines):
                raise ValueError("DocTestFinder.find: __test__ values must be strings, functions, methods, classes, or modules: %r" % (type(item),))
            _boxed_docs(item, name + ".__test__." + attr, owner, found, seen)


def _docstring_lineno(obj, lines, filename):
    if isinstance(obj, types.ModuleType):
        start = 0
    elif isinstance(obj, type):
        if not lines:
            return None
        first = vars(obj).get("__firstlineno__")
        if first is None:
            return None
        start = first - 1
    else:
        if isinstance(obj, property):
            obj = obj.fget
        if hasattr(obj, "__wrapped__"):
            from inspect import unwrap
            obj = unwrap(obj)
        code = getattr(obj, "__code__", None)
        if code is None or (lines and code.co_filename != filename):
            return None
        start = code.co_firstlineno - 1
    if not lines:
        return start + 1
    # DocTestFinder counts from the opening quote, including on a definition line.
    import re
    for index in range(start, len(lines)):
        if re.match(r"(^|.*:)\s*\w*['\"]", lines[index]):
            return index
    return None


class _DocTestSuite(unittest.TestSuite):
    def _removeTestAtIndex(self, index):
        pass


def DocTestSuite(module=None, globs=None, extraglobs=None, *, optionflags=0, setUp=None, tearDown=None, checker=None):
    from _imre_importlib import bind_doctest
    if module is None:
        module = sys.modules[sys._getframemodulename(1)]
    elif isinstance(module, str):
        module = __import__(module, fromlist=["*"])
    elif not isinstance(module, types.ModuleType):
        raise TypeError("Expected a module, string, or None")
    namespace = (module.__dict__ if globs is None else globs).copy()
    if extraglobs is not None:
        namespace.update(extraglobs)
    namespace.setdefault("__name__", "__main__")
    found = {}
    _boxed_docs(module, module.__name__, module, found, set())
    parser = DocTestParser()
    suite = _DocTestSuite()
    filename = getattr(module, "__file__", None) or module.__name__
    if filename.endswith(".pyc"):
        filename = filename[:-1]
    lines = None if filename.endswith((".py", ".pyw")) else []
    for name in sorted(found):
        obj, doc = found[name]
        examples = parser.get_examples(doc, name)
        if examples:
            test = DocTest(name, doc, examples)
            test.globs = namespace.copy()
            test.filename = filename
            bind_doctest(test, module)
            if lines is None:
                import linecache
                lines = linecache.getlines(filename, module.__dict__)
            test.lineno = _docstring_lineno(obj, lines, filename)
            suite.addTest(DocTestCase(test, optionflags, setUp, tearDown, checker))
    return suite


# Object cases store hooks on each instance, including their absence after deletion.
del _DocTestCase._dt_setUp, _DocTestCase._dt_tearDown


class DocTestCase(_DocTestCase):
    def __init__(self, test, optionflags=0, setUp=None, tearDown=None, checker=None):
        super().__init__(test)
        self._dt_optionflags = optionflags
        self._dt_checker = checker
        self._dt_setUp = setUp
        self._dt_tearDown = tearDown


def _execute_case(case, debug):
    execute(case._dt_test, debug, case._dt_optionflags, case._dt_checker)


class DocFileCase(DocTestCase):
    def id(self):
        return self._dt_test.name.replace(".", "_")

    def __repr__(self):
        return self._dt_test.filename

    __str__ = __repr__

    def shortDescription(self):
        return "Doctest: " + self._dt_test.name

    def tearDown(self):
        # File cases restore globals before unittest cleanups even if their hook raises.
        self._tear_down(True)

    def runTest(self):
        test = self._dt_test
        try:
            super().runTest()
        except AssertionError as exc:
            raise self.failureException(
                f'Failed doctest test for {test.name}\n  File "{test.filename}", line 0\n\n{exc}'
            ) from None


def _docfile_package(package):
    if package is None:
        return sys.modules[sys._getframemodulename(2)]
    if isinstance(package, str):
        return __import__(package, fromlist=["*"])
    if not isinstance(package, types.ModuleType):
        raise TypeError("Expected a module, string, or None")
    return package


def _docfile_path(package, path):
    if path.startswith("/"):
        raise ValueError("Module-relative files may not have absolute paths")
    if getattr(package, "__file__", None) is not None:
        return os.path.join(os.path.dirname(package.__file__), path)
    if package.__name__ == "__main__":
        directory = os.path.dirname(sys.argv[0]) if sys.argv and sys.argv[0] else os.curdir
        return os.path.join(directory, path)
    for directory in getattr(package, "__path__", ()):
        filename = os.path.join(directory, path)
        if os.path.exists(filename):
            return filename
    raise ValueError(f"Can't resolve paths relative to the module {package.__name__!r} (it has no __file__)")


def DocFileTest(path, module_relative=True, package=None, globs=None,
                parser=DocTestParser(), encoding=None, **options):
    get_data = None
    if module_relative:
        package = _docfile_package(package)
        path = _docfile_path(package, path)
        loader = getattr(package, "__loader__", None)
        if loader is None:
            loader = getattr(getattr(package, "__spec__", None), "loader", None)
        get_data = getattr(loader, "get_data", None)
    elif package is not None:
        raise ValueError("Package may only be specified for module-relative paths.")
    if get_data is None:
        with open(path, encoding=encoding or "utf-8") as stream:
            text = stream.read()
    else:
        text = get_data(path).decode(encoding or "utf-8").replace("\r\n", "\n").replace("\r", "\n")
    namespace = {} if globs is None else globs.copy()
    namespace.setdefault("__file__", path)
    test = parser.get_doctest(text, namespace, os.path.basename(path), path, 0)
    return DocFileCase(test, **options)


def DocFileSuite(*paths, **options):
    if options.get("module_relative", True):
        options["package"] = _docfile_package(options.get("package"))
    suite = _DocTestSuite()
    for path in paths:
        suite.addTest(DocFileTest(path, **options))
    return suite
