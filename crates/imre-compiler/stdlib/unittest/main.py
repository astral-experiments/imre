"""Run unittest suites with the boxed test loader and runner."""

# Adapted from CPython 3.14.5 Lib/unittest/main.py. See ../docstrings/LICENSE.
import os
import re
import sys

from . import _MainRunner, loader, runner

__unittest = True
_NO_TESTS_EXITCODE = 5
_LONG_OPTIONS = ("--verbose", "--quiet", "--failfast", "--locals")


def _convert_name(name):
    # on Linux / Mac OS X 'foo.PY' is not importable, but on
    # Windows it is. Simpler to do a case insensitive match
    # a better check would be to check that the name is a
    # valid Python module name.
    if os.path.isfile(name) and name.lower().endswith('.py'):
        if os.path.isabs(name):
            rel_path = os.path.relpath(name, os.getcwd())
            if os.path.isabs(rel_path) or rel_path.startswith(os.pardir):
                return name
            name = rel_path
        # on Windows both '\' and '/' are used as path
        # separators. Better to replace both than rely on os.path.sep
        return os.path.normpath(name)[:-3].replace('\\', '.').replace('/', '.')
    return name


def _convert_select_pattern(pattern):
    return pattern if "*" in pattern else "*" + pattern + "*"


def _main_option(arg):
    # argparse recognizes a known option before treating strings with spaces as names.
    if arg[:2] in ("-k", "-v", "-q", "-f"):
        return True
    if "=" in arg and arg.partition("=")[0] in _LONG_OPTIONS:
        return True
    return (arg.startswith("-") and arg != "-" and " " not in arg
            and not re.match(r"-\.?\d", arg))


def _parse_main_args(argv, options):
    names = []
    patterns = []
    extras = []
    names_finished = False
    args = iter(argv[1:])
    for arg in args:
        if arg == "--":
            if names_finished:
                extras.append(arg)
                extras.extend(args)
            else:
                names.extend(args)
            break
        if not _main_option(arg):
            (extras if names_finished else names).append(arg)
            continue
        # The positional `tests` argument is a single group. Options may precede or follow it.
        names_finished = bool(names)
        if arg.startswith("--"):
            flag = next((flag for flag in _LONG_OPTIONS if flag.startswith(arg)), arg)
            if flag == "--locals":
                options.tb_locals = True
            else:
                options._setOption(flag)
            continue
        for i, flag in enumerate(arg[1:], 2):
            if flag != "k":
                options._setOption("-" + flag)
                continue
            pattern = arg[i:]
            if pattern:
                pattern = pattern.removeprefix("=")
            else:
                pattern = next(args, None)
                if pattern is None or _main_option(pattern):
                    print("argument -k: expected one argument", file=sys.stderr)
                    sys.exit(2)
            patterns.append(_convert_select_pattern(pattern))
            break
    if extras:
        print("unrecognized arguments: " + " ".join(extras), file=sys.stderr)
        sys.exit(2)
    return names, patterns


class TestProgram(object):
    """A command-line program that runs a set of tests; this is primarily
       for making test modules conveniently executable.
    """
    # defaults for testing
    module=None
    verbosity = 1
    failfast = catchbreak = buffer = progName = warnings = testNamePatterns = None
    _discovery_parser = None

    def __init__(self, module='__main__', defaultTest=None, argv=None,
                    testRunner=None, testLoader=loader.defaultTestLoader,
                    exit=True, verbosity=1, failfast=None, catchbreak=None,
                    buffer=None, warnings=None, *, tb_locals=False,
                    durations=None):
        if isinstance(module, str):
            self.module = __import__(module)
            for part in module.split('.')[1:]:
                self.module = getattr(self.module, part)
        else:
            self.module = module
        if argv is None:
            argv = sys.argv

        self.exit = exit
        self.failfast = failfast
        self.catchbreak = catchbreak
        self.verbosity = verbosity
        self.buffer = buffer
        self.tb_locals = tb_locals
        self.durations = durations
        if warnings is None and not sys.warnoptions:
            # even if DeprecationWarnings are ignored by default
            # print them anyway unless other warnings settings are
            # specified by the warnings arg or the -W python flag
            self.warnings = 'default'
        else:
            # here self.warnings is set either to the value passed
            # to the warnings args or to None.
            # If the user didn't pass a value self.warnings will
            # be None. This means that the behavior is unchanged
            # and depends on the values passed to -W.
            self.warnings = warnings
        self.defaultTest = defaultTest
        self.testRunner = testRunner
        self.testLoader = testLoader
        self.progName = os.path.basename(argv[0])
        self.parseArgs(argv)
        self.runTests()

    def parseArgs(self, argv):
        if self.module is None and len(argv) > 1 and argv[1].lower() == "discover":
            self._discovery_parser = self._make_discovery_parser()
            self._do_discovery(argv[2:])
            return

        options = _MainRunner()
        options.verbose = self.verbosity
        options.tb_locals = getattr(self, "tb_locals", False)
        options.failfast = False if self.failfast is None else self.failfast
        if self.buffer is None:
            self.buffer = False
        if self.catchbreak is None:
            self.catchbreak = False
        if not hasattr(self, "durations"):
            self.durations = None
        self.tests, patterns = _parse_main_args(argv, options)
        self.tb_locals = options.tb_locals
        self.verbosity = options.verbose
        self.failfast = options.failfast
        if self.testNamePatterns is None or patterns:
            self.testNamePatterns = patterns
        if self.module is None and not self.tests:
            self._discovery_parser = self._make_discovery_parser()
            self._do_discovery([])
            return

        if self.tests:
            self.testNames = [_convert_name(name) for name in self.tests]
        elif isinstance(self.defaultTest, str):
            self.testNames = (self.defaultTest,)
        elif self.defaultTest is not None:
            self.testNames = list(self.defaultTest)
        else:
            self.testNames = None

        self.createTests()

    def _make_discovery_parser(self):
        import argparse

        parser = argparse.ArgumentParser(prog="%s discover" % self.progName)
        parser.add_argument("-v", "--verbose", dest="verbosity", action="store_const", const=2)
        parser.add_argument("-q", "--quiet", dest="verbosity", action="store_const", const=0)
        parser.add_argument("--locals", dest="tb_locals", action="store_true")
        parser.add_argument("--durations", type=int)
        for short, long, attr in (("-f", "--failfast", "failfast"),
                                  ("-c", "--catch", "catchbreak"),
                                  ("-b", "--buffer", "buffer")):
            if getattr(self, attr) is None:
                parser.add_argument(short, long, dest=attr, action="store_true")
                setattr(self, attr, False)
        if self.testNamePatterns is None:
            parser.add_argument("-k", dest="testNamePatterns", action="append",
                                type=_convert_select_pattern)
            self.testNamePatterns = []

        parser.add_argument("-s", "--start-directory", dest="start")
        parser.add_argument("-p", "--pattern", dest="pattern")
        parser.add_argument("-t", "--top-level-directory", dest="top")
        for attr in ("start", "pattern", "top"):
            parser.add_argument(attr, nargs="?", default=argparse.SUPPRESS, help=argparse.SUPPRESS)
        return parser

    def _do_discovery(self, argv, Loader=None):
        self.start, self.pattern, self.top = ".", "test*.py", None
        if argv is not None:
            if self._discovery_parser is None:
                self._discovery_parser = self._make_discovery_parser()
            self._discovery_parser.parse_args(argv, self)
        self.createTests(from_discovery=True, Loader=Loader)

    def createTests(self, from_discovery=False, Loader=None):
        if self.testNamePatterns:
            self.testLoader.testNamePatterns = self.testNamePatterns
        if from_discovery:
            test_loader = self.testLoader if Loader is None else Loader()
            self.test = test_loader.discover(self.start, self.pattern, self.top)
        elif self.testNames is None:
            self.test = self.testLoader.loadTestsFromModule(self.module)
        else:
            self.test = self.testLoader.loadTestsFromNames(self.testNames, self.module)

    def runTests(self):
        if self.catchbreak:
            from .signals import installHandler
            installHandler()
        if self.testRunner is None:
            self.testRunner = runner.TextTestRunner
        if isinstance(self.testRunner, type):
            try:
                try:
                    testRunner = self.testRunner(verbosity=self.verbosity,
                                                 failfast=self.failfast,
                                                 buffer=self.buffer,
                                                 warnings=self.warnings,
                                                 tb_locals=self.tb_locals,
                                                 durations=self.durations)
                except TypeError:
                    # didn't accept the tb_locals or durations argument
                    testRunner = self.testRunner(verbosity=self.verbosity,
                                                 failfast=self.failfast,
                                                 buffer=self.buffer,
                                                 warnings=self.warnings)
            except TypeError:
                # didn't accept the verbosity, buffer or failfast arguments
                testRunner = self.testRunner()
        else:
            # it is assumed to be a TestRunner instance
            testRunner = self.testRunner
        self.result = testRunner.run(self.test)
        if self.exit:
            if not self.result.wasSuccessful():
                sys.exit(1)
            elif self.result.testsRun == 0 and len(self.result.skipped) == 0:
                sys.exit(_NO_TESTS_EXITCODE)
            else:
                sys.exit(0)


main = TestProgram
