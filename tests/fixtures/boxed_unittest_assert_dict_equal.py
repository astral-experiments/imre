from unittest import TestCase


def current():
    case = TestCase()
    compare = case.assertDictEqual
    with case.assertRaises(AssertionError) as first:
        compare(None, {})
    assert str(first.exception) == "None is not an instance of <class 'dict'> : First argument is not a dictionary"
    with case.assertRaises(AssertionError) as second:
        compare(d2=[], d1={})
    assert str(second.exception) == "[] is not an instance of <class 'dict'> : Second argument is not a dictionary"

    class Mapping(dict):
        def __ne__(self, other):
            return False

    assert compare(Mapping({1: "one"}), {}) is None
    with case.assertRaises(AssertionError) as changed:
        compare({1: "one"}, {1: "two"}, msg="changed")
    print(str(changed.exception))

    for first, second, comparator in (
        ({"units": 2}, {"units": 3}, case.assertDictEqual),
        (["x"], ["y"], case.assertListEqual),
        (("x",), ("y",), case.assertTupleEqual),
    ):
        with case.assertRaises(AssertionError) as direct:
            comparator(first, second, msg="report")
        with case.assertRaises(AssertionError) as dispatched:
            case.assertEqual(first, second, msg="report")
        assert dispatched.exception.args == direct.exception.args
