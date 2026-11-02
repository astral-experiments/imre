import subprocess
import sys
import unittest
from test import test_os


def process_environment(label):
    # env(1) reads the child's process environment independently of os.environb.
    completed = subprocess.run(["/usr/bin/env", "-0"], stdout=subprocess.PIPE, check=True)
    print(label + " " + completed.stdout.hex())


def audit_environment(label):
    # Preserve the original outcome even if the platform cannot perform this extra audit.
    try:
        process_environment(label)
    except Exception as error:
        print("AUDIT_ERROR " + label + " " + str(error))


def main():
    tests = unittest.defaultTestLoader.loadTestsFromTestCase(test_os.EnvironTests)
    chosen = unittest.TestSuite()
    for case in tests:
        identifier = case.id()
        print("ID " + identifier)
        if identifier == sys.argv[1]:
            chosen.addTest(case)
    if sys.argv[1] == "--list":
        return
    if chosen.countTestCases() != 1:
        raise AssertionError("expected exactly one selected original")
    print("EXECUTABLE " + sys.executable)
    audit_environment("BEFORE")
    try:
        result = unittest.TextTestRunner(verbosity=2).run(chosen)
    finally:
        audit_environment("AFTER")
    print("RAN " + str(result.testsRun))
    if result.skipped:
        print("OUTCOME SKIP")
    elif result.errors:
        print("OUTCOME ERROR")
    elif result.failures:
        print("OUTCOME FAIL")
    elif result.wasSuccessful():
        print("OUTCOME PASS")
    else:
        print("OUTCOME OTHER")
    if result.testsRun != 1 or not result.wasSuccessful():
        raise SystemExit(1)


main()
