def _checked_example(test: DocTest, index: int, source: str) -> Example:
    example = test.examples[index]
    if example.source != source:
        raise ValueError("doctest source changed after native compilation: " + test.name)
    return example


class _DocTestSuite:
    _tests: list[DocTestCase]

    def __init__(self) -> None:
        self._tests = []

    def __iter__(self) -> Iterator[DocTestCase]:
        yield from self._tests

    def countTestCases(self) -> int:
        return len(self._tests)

    def debug(self) -> None:
        for case in self._tests:
            case.debug()


class _ModuleDocs:
    _docs: list[tuple[str, str]]

    def __init__(self, docs: list[tuple[str, str]]) -> None:
        self._docs = docs


_modules: dict[str, _ModuleDocs] = {}


def _module_docs(name: str, docs: list[tuple[str, str]]) -> _ModuleDocs:
    if name not in _modules:
        _modules[name] = _ModuleDocs(docs)
    return _modules[name]


def DocTestSuite(module: _ModuleDocs) -> _DocTestSuite:
    # The compiler supplies the selected module's own docstrings in name order.
    # Parsing and building cases happen at the point of the Python call.
    suite = _DocTestSuite()
    parser = DocTestParser()
    for name, docstring in module._docs:
        examples = parser.get_examples(docstring, name)
        if examples:
            suite._tests.append(DocTestCase(DocTest(name, docstring, examples)))
    return suite
