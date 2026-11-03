from unittest import TestCase


class Failure(AssertionError):
    pass


class SparseSequence:
    def __len__(self):
        return 2

    def __getitem__(self, index):
        if index == 0:
            return 1
        raise IndexError(index)

    def __repr__(self):
        return "SparseSequence()"


def current():
    case = TestCase()
    case.failureException = Failure
    assert case.assertListEqual(list1=[1], list2=[1]) is None
    with case.assertRaises(Failure) as caught:
        case.assertListEqual([1], (1,))
    assert str(caught.exception) == "Second sequence is not a list: (1,)"
    with case.assertRaises(Failure) as caught:
        case.assertListEqual([1], [2], msg="changed list")
    print(str(caught.exception))
    assert case.assertTupleEqual(tuple1=(1,), tuple2=(1,)) is None
    with case.assertRaises(Failure) as caught:
        case.assertTupleEqual([1], (1,))
    assert str(caught.exception) == "First sequence is not a tuple: [1]"
    with case.assertRaises(Failure) as caught:
        case.assertTupleEqual((1,), (2,), msg="changed tuple")
    print(str(caught.exception))

    calls = []

    class Routed(TestCase):
        def assertSequenceEqual(self, first, second, msg, *, seq_type):
            calls.append((first, second, msg, seq_type))
            return "ignored"

    assert Routed().assertTupleEqual((1,), (2,), "custom") is None
    assert calls == [((1,), (2,), "custom", tuple)]
    compare = case.assertSequenceEqual
    assert compare([], ()) is None
    assert compare([1, 2], (1, 2)) is None
    assert compare((1,), (1,), seq_type=tuple) is None
    words = ["long item " + str(index) for index in range(8)]
    for left, right, seq_type in (
        ([1], [1, 2], list),
        ([words, {"z": 1, "a": 2}], [words, {"z": 1, "a": 3}], None),
        ((1, 2), (3,), tuple),
        ([1], (1,), list),
        ((1,), [1], list),
        (1, [], None),
        ([], 1, None),
        (SparseSequence(), [1, 2], None),
        ([1, 2], SparseSequence(), None),
    ):
        with case.assertRaises(Failure) as caught:
            compare(left, right, msg="detail", seq_type=seq_type)
        print(str(caught.exception))

    for limit in (0, 11, 12, None):
        case.maxDiff = limit
        with case.assertRaises(Failure) as caught:
            compare([1], [2])
        print(str(caught.exception))

    case.longMessage = False
    marker = ["custom message"]
    with case.assertRaises(Failure) as caught:
        compare("ab", "ac", marker)
    assert caught.exception.args[0] is marker
