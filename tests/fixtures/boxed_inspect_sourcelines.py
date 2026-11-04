from inspect import getsource, getsourcelines


def identity(func):
    return func


def current():
    @identity
    def sample(
        value,
    ):
        return value
        # Inside the block.

    expected = [
        "    @identity\n",
        "    def sample(\n",
        "        value,\n",
        "    ):\n",
        "        return value\n",
        "        # Inside the block.\n",
    ]
    with open(__file__, encoding="utf-8") as source:
        first = source.readlines().index(expected[0]) + 1
    assert getsourcelines(sample) == (expected, first)

    def wrapper():
        pass

    wrapper.__wrapped__ = sample
    assert getsourcelines(wrapper) == (expected, first)
    assert getsource(wrapper) == "".join(expected)

    namespace = {"__name__": "_missing_inspect_module"}
    exec(compile("def missing(): pass", "_missing_inspect_source.py", "exec"), namespace)
    try:
        getsourcelines(namespace["missing"])
    except OSError as exc:
        assert str(exc) == "source code not available"
    else:
        raise AssertionError("expected unavailable source")
