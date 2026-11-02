import os
import unittest


class Payload:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value


class Reader(unittest.TestCase):
    def read(self) -> int:
        return self.payload.value


def broken_payload() -> Payload:
    raise ValueError("fixture right-hand side")


class Interrupted(Reader):
    def setUp(self):
        self.payload = broken_payload()


class Partial(Reader):
    def setUp(self):
        self.payload = Payload(99)
        raise ValueError("after assignment")


class LabelReader(unittest.TestCase):
    def read_label(self) -> str:
        return self.label


class WithDefault(LabelReader):
    label = "class default"

    def setUp(self):
        self.label = "instance value"

    def own_label(self) -> str:
        return self.label


class OtherDefault(LabelReader):
    label = "other default"


class Ready(Reader):
    def setUp(self):
        self.payload = Payload(45)

    def test_read(self):
        self.assertEqual(self.read(), 45)
        self.payload = Payload(12)
        self.assertEqual(self.read(), 12)
        print("ready body")

    def tearDown(self):
        print("ready teardown")


class Failed(Reader):
    def setUp(self):
        print("failing setup")
        raise RuntimeError("setup failed")

    def test_never(self):
        raise AssertionError("body after failed setup")

    def tearDown(self):
        raise AssertionError("teardown after failed setup")


class Skipped(Reader):
    def setUp(self):
        print("skipping setup")
        raise unittest.SkipTest("fixture unavailable")

    def test_never(self):
        raise AssertionError("body after skipped setup")

    def tearDown(self):
        raise AssertionError("teardown after skipped setup")


class Conditional(Reader):
    def setUp(self):
        if self.enabled():
            self.payload = Payload(7)

    def enabled(self) -> bool:
        return False

    def test_absent(self):
        try:
            self.read()
        except AttributeError as exc:
            print(str(exc))
        else:
            raise AssertionError("unassigned fixture field was present")


class BranchTaken(Conditional):
    def enabled(self) -> bool:
        return True

    def test_absent(self):
        self.assertEqual(self.read(), 7)
        print("branch assigned")


class TextSave(unittest.TestCase):
    def setUp(self):
        self.__save = dict(os.environ)

    def original(self) -> str:
        return self.__save["IMRE_SETUP_FIXTURE"]


class ByteSave(TextSave):
    def setUp(self):
        if os.supports_bytes_environ:
            self.__save = dict(os.environb)

    def original_bytes(self) -> bytes:
        return self.__save[b"IMRE_SETUP_FIXTURE"]


def outside_runner() -> None:
    saved = dict(os.environ)
    os.environ["IMRE_SETUP_FIXTURE"] = "initial"
    text = TextSave()
    try:
        text.original()
    except AttributeError as exc:
        print(str(exc))
    text.setUp()
    os.environ["IMRE_SETUP_FIXTURE"] = "replacement"
    print(text.original())
    other = ByteSave()
    other.setUp()
    if os.supports_bytes_environ:
        print(other.original_bytes())
    try:
        other.original()
    except AttributeError as exc:
        print(str(exc))
    os.environ.clear()
    os.environ.update(saved)
    direct = Ready()
    try:
        direct.read()
    except AttributeError as exc:
        print(str(exc))
    else:
        raise AssertionError("constructing a case ran its setup")
    interrupted = Interrupted()
    try:
        interrupted.setUp()
    except ValueError as exc:
        print(str(exc))
    try:
        interrupted.read()
    except AttributeError as exc:
        print(str(exc))
    else:
        raise AssertionError("failed right-hand side published the field")
    direct.setUp()
    try:
        direct.payload = broken_payload()
    except ValueError:
        print(direct.read())
    partial = Partial()
    try:
        partial.setUp()
    except ValueError:
        print(partial.read())
    first = WithDefault()
    fresh = WithDefault()
    print(first.read_label())
    print(first.own_label())
    print(OtherDefault().read_label())
    first.setUp()
    print(first.read_label())
    print(first.own_label())
    print(fresh.read_label())
    print(WithDefault.label)


if __name__ == "__main__":
    outside_runner()
    unittest.main()
