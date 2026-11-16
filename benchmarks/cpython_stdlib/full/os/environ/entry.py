import os
import subprocess
import sys
import unittest
from importlib import import_module

test_os = import_module("test.test_os")


def process_environment(label):
    # env(1) reads the child's process environment independently of os.environb.
    completed = subprocess.run(["/usr/bin/env", "-0"], stdout=subprocess.PIPE, check=True)
    print(label + " " + repr(completed.stdout))


def audit_environment(label):
    # Preserve the original outcome even if the platform cannot perform this extra audit.
    try:
        process_environment(label)
    except Exception as error:
        print("AUDIT_ERROR " + label + " " + str(error))


def mapping_snapshot(name, phase):
    try:
        return dict(getattr(os, name))
    except Exception as error:
        print("MAPPING_AUDIT_ERROR " + name + " " + phase + " " + type(error).__name__)
        return None


def report_mapping(name, original):
    restored = mapping_snapshot(name, "after")
    status = "unobserved"
    if original is not None and restored is not None:
        try:
            status = "same" if original == restored else "different"
        except Exception as error:
            print("MAPPING_AUDIT_ERROR " + name + " comparison " + type(error).__name__)
    print("MAPPING_" + name + " " + status)


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
    original_text = mapping_snapshot("environ", "before")
    original_bytes = mapping_snapshot("environb", "before")
    try:
        result = unittest.TextTestRunner(verbosity=2).run(chosen)
    finally:
        audit_environment("AFTER")
        report_mapping("environ", original_text)
        report_mapping("environb", original_bytes)
    print("RAN " + str(result.testsRun))
    if result.skipped:
        print("OUTCOME SKIP")
        for _, reason in result.skipped:
            print("SKIP_REASON " + reason)
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
