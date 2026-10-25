"""Observe the queue display and both products in the untouched CPython method."""

import inspect
import itertools
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import load_suite, verify_files
from suite_native import run_in_fixtures


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("use a test-enabled CPython 3.14.5 build")
    import test
    from test import test_logging

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(Path(test.__file__).resolve().parents[2],
                 manifest["files"] | manifest["reference_files"])
    cls = test_logging.ConfigDictTest
    test_code = inspect.unwrap(cls.test_config_queue_handler).__code__
    helper = cls.do_queuehandler_configuration.__code__
    case_id = f"{cls.__module__}.{cls.__name__}.test_config_queue_handler"

    def observe(case, method):
        seen = []
        expected = []

        def profile(frame, event, arg):
            if event != "call" or frame.f_code is not helper:
                return
            assert frame.f_back.f_code is test_code
            local = frame.f_back.f_locals
            if len(seen) == 0:
                queues, listeners = local["qvalues"], local["lvalues"]
                assert type(queues) is tuple and len(queues) == 7 and len(listeners) == 4
                assert queues[:3] == (None, f"{cls.__module__}.queueMaker",
                                      f"{cls.__module__}.CustomQueue")
                assert all(a is b for a, b in zip(queues[3:], local["dqs"] + local["qs"], strict=True))
                expected.extend(itertools.product(queues, listeners))
            elif len(seen) == 28:
                expected.extend((q, listener) for q, listener in
                                itertools.product(local["qvalues"], local["lvalues"])
                                if q is not None or listener is not None)
            q, listener = expected[len(seen)]
            assert frame.f_locals["qspec"] is q and frame.f_locals["lspec"] is listener
            seen.append(None)

        previous = sys.getprofile()
        sys.setprofile(profile)
        try:
            method()
        finally:
            sys.setprofile(previous)
        assert len(seen) == len(expected) == 52
        return {"body": "cpython", "fixtures": "cpython", "operations": "cpython",
                "valid_pairs": 28, "invalid_pairs": 24, "expanded_elements_identical": 4}

    observed, errors, skips = run_in_fixtures(load_suite(manifest["module"]), {case_id: observe})
    if errors or skips:
        raise RuntimeError((errors, skips))
    print(json.dumps(observed, indent=2))


if __name__ == "__main__":
    main()
