from unittest import TestCase, main


class Outcomes(TestCase):
    def setUp(self) -> None:
        print("setup")

    def tearDown(self) -> None:
        print("teardown")

    def test_bad(self) -> None:
        self.fail("deliberate failure")

    def test_error(self) -> None:
        raise ValueError("deliberate error")

    def test_exit(self) -> None:
        raise SystemExit(7)

    def test_ok(self) -> None:
        print("passed body")

    def test_skip(self) -> None:
        self.skipTest("platform")


class SetupError(TestCase):
    def setUp(self) -> None:
        print("setup error")
        raise RuntimeError("before")

    def tearDown(self) -> None:
        print("do not run after failed setup")

    def test_body(self) -> None:
        print("do not run after failed setup")


class TearError(TestCase):
    def tearDown(self) -> None:
        print("teardown error")
        raise RuntimeError("after")

    def test_body(self) -> None:
        self.fail("failed body")


if __name__ == "__main__":
    main()
