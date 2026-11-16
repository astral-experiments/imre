def _checked_example(test: DocTest, index: int, source: str) -> Example:
    example = test.examples[index]
    if example.source != source:
        raise ValueError("doctest source changed after native compilation: " + test.name)
    return example


class _DocTestSuite:
    _tests: list[DocTestCase]

    def __init__(self) -> None:
        self._tests = []

    def addTest(self, case: DocTestCase) -> None:
        self._tests.append(case)

    def __iter__(self) -> Iterator[DocTestCase]:
        yield from self._tests

    def countTestCases(self) -> int:
        return len(self._tests)

    def debug(self) -> None:
        for case in self._tests:
            case.debug()


class _ModuleDocs:
    _filename: str | None
    _docs: list[tuple[str, str, int | None]]

    def __init__(self, filename: str | None, docs: list[tuple[str, str, int | None]]) -> None:
        self._filename = filename
        self._docs = docs


_modules: dict[str, _ModuleDocs] = {}


def _module_docs(name: str, filename: str | None, docs: list[tuple[str, str, int | None]]) -> _ModuleDocs:
    if name not in _modules:
        _modules[name] = _ModuleDocs(filename, docs)
    return _modules[name]


def _default_case(test: DocTest) -> DocTestCase:
    return DocTestCase(test)


def DocTestSuite(module: _ModuleDocs) -> _DocTestSuite:
    parser = DocTestParser()
    tests: list[DocTest] = []
    for name, docstring, lineno in module._docs:
        tests.append(parser.get_doctest(docstring, {}, name, module._filename, lineno))
    return _suite_from_tests(tests, _default_case)
