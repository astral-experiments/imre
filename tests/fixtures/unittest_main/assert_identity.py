import unittest


class Display:
    label: str
    log: list[str]

    def __init__(self, label: str, log: list[str]) -> None:
        self.label = label
        self.log = log

    def __repr__(self) -> str:
        self.log.append(self.label)
        return self.label


def operand(log: list[str], tag: str, value: Display) -> Display:
    log.append(tag)
    return value


def message(log: list[str]) -> str:
    log.append("msg")
    return "custom"


def broken(log: list[str]) -> str:
    log.append("raised")
    raise ValueError("while binding msg")


class BrokenDisplay:
    def __repr__(self) -> str:
        raise ValueError("failed repr")


class InterruptedDisplay:
    def __repr__(self) -> str:
        raise SystemExit("display interrupted")


class IdentityTest(unittest.TestCase):
    def test_alias_and_equal_distinct_objects(self):
        first = [1]
        alias = first
        self.assertIs(first, alias)
        self.assertIsNot(first, [1])
        self.assertIsNot([], [])
        with self.assertRaises(AssertionError) as empty:
            self.assertIs({}, {})
        self.assertEqual(str(empty.exception), "{} is not {}")
        with self.assertRaises(AssertionError) as caught:
            self.assertIs(first, [1])
        self.assertEqual(str(caught.exception), "[1] is not [1]")
        with self.assertRaises(AssertionError) as same:
            self.assertIsNot(alias, first, "same")
        self.assertEqual(str(same.exception), "unexpectedly identical: [1] : same")

    def test_argument_and_repr_order(self):
        log: list[str] = []
        left = Display("left", log)
        right = Display("right", log)
        self.assertIsNot(expr2=operand(log, "second", right), msg=message(log),
                         expr1=operand(log, "first", left))
        self.assertEqual(log, ["second", "msg", "first"])
        log.clear()
        with self.assertRaises(AssertionError) as caught:
            self.assertIs(expr2=operand(log, "second", right), msg=message(log),
                          expr1=operand(log, "first", left))
        self.assertEqual(log, ["second", "msg", "first", "left", "right"])
        self.assertEqual(str(caught.exception), "left is not right : custom")
        log.clear()
        with self.assertRaises(AssertionError) as identical:
            self.assertIsNot(left, left)
        self.assertEqual(log, ["left"])
        self.assertEqual(str(identical.exception), "unexpectedly identical: left")
        log.clear()
        with self.assertRaises(ValueError) as binding:
            self.assertIs(operand(log, "first", left), operand(log, "second", right), broken(log))
        self.assertEqual(log, ["first", "second", "raised"])
        self.assertEqual(str(binding.exception), "while binding msg")

    def test_stored_methods_keep_their_receiver(self):
        other = unittest.TestCase()
        other.longMessage = False
        same = other.assertIs
        different = self.assertIsNot
        values = [1]
        same(expr2=values, expr1=values)
        different(values, [1])
        with self.assertRaises(AssertionError) as caught:
            same(values, [1], "bound receiver")
        self.assertEqual(str(caught.exception), "bound receiver")
        self.assertRaises(AssertionError, different, values, values)

    def test_failed_repr_falls_back_to_same_object(self):
        value = BrokenDisplay()
        with self.assertRaises(AssertionError) as same:
            self.assertIsNot(value, value)
        text = str(same.exception)
        self.assertTrue(text.startswith("unexpectedly identical: <__main__.BrokenDisplay object at 0x"))
        self.assertTrue(text.endswith(">"))
        original_repr = text[len("unexpectedly identical: "):]
        with self.assertRaises(AssertionError) as different:
            self.assertIs(value, BrokenDisplay())
        self.assertTrue(str(different.exception).startswith(original_repr + " is not <__main__.BrokenDisplay object at 0x"))
        self.assertFalse(str(different.exception).endswith(original_repr))
        interrupted = InterruptedDisplay()
        self.assertIs(interrupted, interrupted)
        with self.assertRaises(SystemExit) as stop:
            self.assertIsNot(interrupted, interrupted)
        self.assertEqual(str(stop.exception), "display interrupted")

    def test_short_message_and_singletons(self):
        self.assertIs(None, None)
        self.assertIsNot(1, True)
        self.assertIsNot(False, 0)
        self.longMessage = False
        with self.assertRaises(AssertionError) as short:
            self.assertIs(True, False, msg="only")
        self.assertEqual(str(short.exception), "only")
        with self.assertRaises(AssertionError) as empty:
            self.assertIsNot((), (), msg="")
        self.assertEqual(str(empty.exception), "unexpectedly identical: ()")


class DifferentFailure(unittest.TestCase):
    failureException = ValueError

    def _raise_identity_failure(self) -> None:
        raise RuntimeError("private implementation names do not override the assertion")

    def test_selected_failure_type(self):
        with self.assertRaises(ValueError) as caught:
            self.assertIs(True, False)
        self.assertEqual(str(caught.exception), "True is not False")


class Override(unittest.TestCase):
    def assertIs(self, expr1: int, expr2: int) -> None:
        self.assertEqual(expr1 + 1, expr2)

    def test_subclass_override(self):
        self.assertIs(1, 2)


unittest.main()
