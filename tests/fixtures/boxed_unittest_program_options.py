import io
import unittest
from contextlib import redirect_stderr


class Example(unittest.TestCase):
    def test_leaf(self):
        self.fail("rejected arguments reached the runner")


def current():
    attempts = []

    class Remember(unittest.TestProgram):
        def parseArgs(self, argv):
            attempts.append(self)
            super().parseArgs(argv)

    case = unittest.TestCase()
    with redirect_stderr(io.StringIO()):
        with case.assertRaises(SystemExit) as grouped:
            Remember(module=__name__, argv=["suite", "Example.test_leaf", "-bf"],
                     failfast=False, exit=False)
        assert grouped.exception.code == 2
        assert attempts[-1].failfast is False

        with case.assertRaises(SystemExit) as first:
            Remember(module=__name__, argv=["suite", "--unknown"], exit=False)
        assert first.exception.code == 2
        program = attempts[-1]
        assert program.buffer is False
        with case.assertRaises(SystemExit) as retry:
            program.parseArgs(["suite", "-b", "Example.test_leaf"])
        assert retry.exception.code == 2
        assert program.buffer is False
