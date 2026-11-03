# The boxed importer discovers docs from the live module instead of a fixed module graph.
import os
import sys
import types


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
        found[name] = doc
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


class _DocTestSuite(unittest.TestSuite):
    def _removeTestAtIndex(self, index):
        pass


def DocTestSuite(module=None, globs=None, extraglobs=None):
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
    for name in sorted(found):
        doc = found[name]
        examples = parser.get_examples(doc, name)
        if examples:
            test = DocTest(name, doc, examples)
            test.globs = namespace.copy()
            suite.addTest(DocTestCase(test))
    return suite


def _case_setup(self):
    self._dt_globs = self._dt_test.globs.copy()


def _case_teardown(self):
    globs = self._dt_test.globs
    globs.clear()
    globs.update(self._dt_globs)


def _case_debug(self):
    self.setUp()
    execute(self._dt_test, True)
    self.tearDown()


DocTestCase.setUp = _case_setup
DocTestCase.tearDown = _case_teardown
DocTestCase.debug = _case_debug


class DocFileCase(DocTestCase):
    def __init__(self, test, optionflags=0, setUp=None, tearDown=None, checker=None):
        super().__init__(test)
        self._dt_optionflags = optionflags
        self._dt_checker = checker
        self._dt_setUp = setUp
        self._dt_tearDown = tearDown

    def id(self):
        return self._dt_test.name.replace(".", "_")

    def __repr__(self):
        return self._dt_test.filename

    __str__ = __repr__

    def shortDescription(self):
        return "Doctest: " + self._dt_test.name

    def setUp(self):
        super().setUp()
        if self._dt_setUp is not None:
            self._dt_setUp(self._dt_test)

    def tearDown(self):
        try:
            if self._dt_tearDown is not None:
                self._dt_tearDown(self._dt_test)
        finally:
            super().tearDown()

    def runTest(self):
        test = self._dt_test
        try:
            execute(test, False, self._dt_optionflags, self._dt_checker)
        except AssertionError as exc:
            raise self.failureException(
                f'Failed doctest test for {test.name}\n  File "{test.filename}", line 0\n\n{exc}'
            ) from None

    def debug(self):
        self.setUp()
        execute(self._dt_test, True, self._dt_optionflags, self._dt_checker)
        self.tearDown()


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
