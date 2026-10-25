from sys import argv
from unittest import TestCase

case = TestCase()
case.longMessage = argv[2] == "long"
msg: str | None = None
if argv[3] == "message":
    msg = argv[4]

operation = argv[1]
try:
    if operation == "assertTrue":
        case.assertTrue(argv[5] == "True", msg)
    elif operation == "assertFalse":
        case.assertFalse(argv[5] == "True", msg)
    else:
        first = int(argv[5])
        second = int(argv[6])
        if operation == "assertLess":
            case.assertLess(first, second, msg)
        elif operation == "assertLessEqual":
            case.assertLessEqual(first, second, msg)
        elif operation == "assertGreater":
            case.assertGreater(first, second, msg)
        elif operation == "assertGreaterEqual":
            case.assertGreaterEqual(first, second, msg)
        elif operation == "assertNotEqual":
            case.assertNotEqual(first, second, msg)
        elif operation == "_baseAssertEqual":
            case._baseAssertEqual(first, second, msg)
        else:
            raise ValueError("unknown assertion")
except AssertionError as exc:
    print("failure")
    print(str(exc))
else:
    print("success")
