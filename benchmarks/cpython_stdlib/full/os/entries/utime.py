"""Run the original UtimeTests case objects with their unittest fixtures."""
import os
import sys
import unittest
from test import support

support.use_resources = []
from test import test_os

loader = unittest.TestLoader()
cases = {}
for case in loader.loadTestsFromTestCase(test_os.UtimeTests):
    ident = case.id()
    if ident in cases:
        raise ValueError("duplicate registration " + ident)
    cases[ident] = case
    print("REGISTER", ident)
if loader.errors or len(cases) != 13:
    raise RuntimeError("incomplete UtimeTests discovery")
print("REGISTERED_CLASS", len(cases))

arguments = sys.argv[1:]
if arguments == ["list"]:
    raise SystemExit(0)

controls = {
    "lose-one-ns": "test_utime",
    "wrong-mtime": "test_utime_by_times",
    "wrong-fd": "test_utime_fd",
    "wrong-dir-fd": "test_utime_dir_fd",
}
control = arguments[0] if arguments and arguments[0] in controls else None
if control:
    method = controls[control]
    if arguments[1:] not in ([method], ["test.test_os.UtimeTests." + method]):
        raise ValueError(control + " requires " + method)
    arguments = arguments[1:]

utime = os.utime
if control == "lose-one-ns":
    def lose_one_ns(path, *, ns):
        return utime(path, ns=(ns[0] - 1, ns[1]))
    os.utime = lose_one_ns
elif control == "wrong-mtime":
    def wrong_mtime(path, *, times):
        return utime(path, times=(times[0], times[1] + 1))
    os.utime = wrong_mtime
elif control == "wrong-fd":
    def wrong_fd(fd, *, ns):
        with open("decoy-fd", "xb", 0) as decoy:
            try:
                return utime(decoy.fileno(), ns=ns)
            finally:
                os.unlink("decoy-fd")
    os.utime = wrong_fd
elif control == "wrong-dir-fd":
    def wrong_dir_fd(path, *, dir_fd, ns):
        with open(path, "xb", 0):
            try:
                return utime(path, ns=ns)
            finally:
                os.unlink(path)
    os.utime = wrong_dir_fd

suite = unittest.TestSuite()
for ident in arguments or cases:
    if ident not in cases:
        ident = "test.test_os.UtimeTests." + ident
    suite.addTest(cases[ident])
    print("SELECT", ident)
result = unittest.TextTestRunner(verbosity=2).run(suite)
print("RESULT", result.testsRun, len(result.failures), len(result.errors), len(result.skipped))
if not result.wasSuccessful():
    raise SystemExit(1)
