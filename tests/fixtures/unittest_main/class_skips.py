import os
import sys
import unittest


def reason(text: str) -> str:
    print("decorating " + text)
    return text


windows = sys.platform == "win32"


@unittest.skipUnless(windows, reason("requires Windows"))
class PlatformCase(unittest.TestCase):
    def setUp(self) -> None:
        print("setup")

    def test_platform(self) -> None:
        print("test body")

    def tearDown(self) -> None:
        print("teardown")


@unittest.skipIf(False, reason("unused reason"))
class InheritedCase(PlatformCase):
    pass


@unittest.skipIf(True, reason("subclass reason"))
@unittest.skipUnless(False, reason("inner reason"))
class OverriddenCase(PlatformCase):
    pass


@unittest.skipUnless(True, reason("also unused"))
class OrdinaryCase(unittest.TestCase):
    @unittest.skipUnless(True, "enabled")
    @unittest.skipIf(False, "available")
    def test_runs(self) -> None:
        print("ran ordinary case")


shell = "/bin/sh"


@unittest.skipUnless(shell and os.path.exists(shell), reason("requires shell"))
class ShellCase(unittest.TestCase):
    def test_shell(self):
        print("shell body")


def unreachable() -> bool:
    raise AssertionError("evaluated short-circuited operand")


@unittest.skipIf(shell or unreachable(), reason("shell present"))
class SkippedShellCase(unittest.TestCase):
    def test_unreachable(self):
        self.fail("shell path is nonempty")


if __name__ == "__main__":
    unittest.main()
