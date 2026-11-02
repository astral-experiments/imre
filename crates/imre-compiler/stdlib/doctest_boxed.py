# The boxed importer discovers docs from the live module instead of a fixed module graph.
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
