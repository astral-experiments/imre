"""Run CPython 3.14.5's unchanged BytesTest.test_contains with native membership."""

import argparse
import hashlib
import inspect
import subprocess
import sys
import tempfile
from pathlib import Path
from test.test_bytes import BytesTest


class Haystack:
    def __init__(self, data, contains):
        self.data = data
        self.contains = contains

    def __contains__(self, needle):
        return self.contains(self.data, needle)


def run_original(contains):
    case = BytesTest("test_contains")
    case.type2test = lambda data: Haystack(data, contains)
    return case.run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    args = parser.parse_args()
    # Lib/test/test_bytes.py in CPython v3.14.5 (PSF license).
    assert sys.version_info[:3] == (3, 14, 5)
    source = inspect.getsource(BytesTest.test_contains).encode()
    assert hashlib.sha256(source).hexdigest() == (
        "634d8c38fabe7e3898e286ef5117124432e354f13fbc9a1cf45bed3ad8500025"
    )
    assert BytesTest("test_contains").run().wasSuccessful()
    queries = []

    def record(data, needle):
        queries.append((data, needle))
        return needle in data

    assert run_original(record).wasSuccessful()
    native = [(data, needle) for data, needle in queries if type(needle) is not bytearray]
    assert len(queries) == 31 and len(native) == 20
    with tempfile.TemporaryDirectory() as directory:
        project = Path(directory)
        program = project / "membership.py"
        binary = project / "membership"
        program.write_text(
            "".join(
                f"try:\n    print({needle!r} in {data!r})\n"
                'except TypeError as e:\n    print("TypeError:" + str(e))\n'
                'except ValueError as e:\n    print("ValueError:" + str(e))\n'
                for data, needle in native
            )
        )
        for mode in ([], ["--reachable-only"]):
            for opt in ("0", "3"):
                subprocess.run(
                    [
                        args.compiler, "build", program, "-o", binary,
                        "--runtime", args.runtime, "--opt-level", opt, *mode,
                    ],
                    check=True,
                    capture_output=True,
                )
                result = subprocess.run([binary], check=True, capture_output=True, text=True)
                outputs = result.stdout.splitlines()
                assert not result.stderr and len(outputs) == len(native)
                def check(provided):
                    answers = iter(provided)
                    seen = []

                    def contains(data, needle):
                        seen.append((data, needle))
                        if type(needle) is bytearray:
                            return needle in data  # No native mutable buffer representation.
                        value = next(answers)
                        if value in ("True", "False"):
                            return value == "True"
                        kind, message = value.split(":", 1)
                        raise {"TypeError": TypeError, "ValueError": ValueError}[kind](message)

                    return run_original(contains), seen

                checked, seen = check(outputs)
                assert checked.wasSuccessful(), (checked.errors, checked.failures)
                assert seen == queries
                flipped = [*outputs]
                flipped[0] = "False"  # Original assertIn must notice.
                assert len(check(flipped)[0].failures) == 1
                wrong_error = [*outputs]
                wrong_error[3] = "TypeError:wrong class"  # Original assertRaises must notice.
                assert len(check(wrong_error)[0].errors) == 1
                label = "reachable" if mode else "whole"
                print(f"{label} O{opt}: 20 native queries, 11 CPython bytearray queries")


if __name__ == "__main__":
    main()
