"""Differential controls for native assertions; not counted as upstream IDs."""
import unittest


class CustomFailure(AssertionError):
    pass


class CustomCase(unittest.TestCase):
    failureException = CustomFailure


def failure_type(case: unittest.TestCase) -> None:
    print(f"{case.failureException!r} / {case.failureException!s}")


def mark(seen: list[str], name: str) -> str:
    seen.append(name)
    return "same"


def text_failure(case: unittest.TestCase, a: str, b: str, msg: str | None = None) -> None:
    try:
        case.assertEqual(first=a, second=b, msg=msg)
    except AssertionError as exc:
        print(repr(str(exc)))
    else:
        raise RuntimeError("unequal text passed")


def list_failure(case: unittest.TestCase, a: list[str], b: list[str]) -> None:
    try:
        case.assertEqual(a, b)
    except AssertionError as first:
        standard = str(first)
    else:
        raise RuntimeError("unequal lists passed")
    try:
        case.assertListEqual(list2=b, list1=a)
    except AssertionError as second:
        if standard != str(second):
            raise RuntimeError("named list assertion differs from assertEqual")
        print(repr(standard))
    else:
        raise RuntimeError("assertListEqual missed a difference")


case = unittest.TestCase()
for value in (10 ** 79, 10 ** 80):
    try:
        case.assertEqual(value, value + 1)
    except AssertionError as exc:
        print(str(exc))
failure_type(case)
failure_type(CustomCase())
custom = CustomCase()
try:
    custom.assertEqual("incorrect", "correct")
except custom.failureException as custom_error:
    print(repr(str(custom_error)))
else:
    raise RuntimeError("subclass's failureException was not raised")
try:
    raise ValueError("crossed")
except custom.failureException:
    raise RuntimeError("subclass's failureException caught ValueError")
except ValueError as crossed:
    print(str(crossed))
seen: list[str] = []
case.assertEqual(second=mark(seen, "second"), first=mark(seen, "first"))
print(",".join(seen))
case.assertEqual("été 🚀", "été 🚀")
case.assertEqual(["Ruby", "Tcl"], ["Ruby", "Tcl"])
case.assertEqual([], [])
case.assertListEqual([b"Ruby", b"Tcl"], [b"Ruby", b"Tcl"])
case.assertListEqual([3, 8], [3, 8])
text_failure(case, "abc", "acb")
text_failure(case, "laden swallows\nfly sloely", "laden swallows\nfly slowly")
text_failure(case, "laden swallows\nfly sloely\n", "laden swallows\nfly slowly", "ending")
text_failure(case, "", "A\r\nB\u2028C\x85")
text_failure(case, "α" * 100 + "é", "α" * 100 + "中")
list_failure(case, ["red", "blue"], ["red", "bleu"])
list_failure(case, [], ["ground"])
list_failure(case, ["left", "trailing"], [])
list_failure(case, ["hello " * 18 + "end", "δ"], ["hello " * 18 + "and", "δ"])
try:
    case.assertEqual([b"\xff"], [b"\x00"])
except AssertionError as bytes_error:
    print(repr(str(bytes_error)))
else:
    raise RuntimeError("unequal bytes lists passed")
try:
    case.assertListEqual([3, 8], [3, 9])
except AssertionError as int_error:
    print(repr(str(int_error)))
else:
    raise RuntimeError("unequal int lists passed")
case.longMessage = False
text_failure(case, "before", "after", "short message")
case.longMessage = True
case.maxDiff = 0
text_failure(case, "before", "after", "truncated")
list_failure(case, ["before"], ["after"])
case.maxDiff = None
text_failure(case, "before", "after", "unlimited")
case._diffThreshold = 2
text_failure(case, "long text", "other long text")
