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


def outside_runner() -> None:
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


if __name__ == "__main__":
    outside_runner()
    unittest.main()
