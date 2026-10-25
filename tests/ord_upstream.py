"""Run the pinned CPython ord assertions with native values in both compiler modes."""
import argparse
from pathlib import Path
import hashlib
import inspect
import subprocess
import sys
import tempfile
from unittest.mock import patch
from test import test_builtin, test_bytes


def original(module, case, replacement):
    # Every call to the builtin in the unchanged method uses the supplied implementation.
    with patch.object(module, "ord", replacement, create=True):
        return case("test_ord").run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    assert sys.version_info[:3] == (3, 14, 5)
    cases = [(test_builtin, test_builtin.BuiltinTest, "2d9a03c0598c9aec32cd37e56e120bf81eca61adf2ea46a36b1f27f9d65f8b35"),
             (test_bytes, test_bytes.BytesTest, "0e5905164db420e85b43a3cbcb20c1d779e6e966b2df06b1d83008c2ae7d651e")]
    inputs = []
    expected = []
    for module, klass, digest in cases:
        method = inspect.getsource(klass.test_ord)
        assert hashlib.sha256(method.encode()).hexdigest() == digest
        def record(value):
            inputs.append(value)
            try:
                result = ord(value)
            except TypeError as error:
                expected.append("T" + str(error))
                raise
            expected.append("I" + str(result))
            return result
        assert original(module, klass, record).wasSuccessful()
    assert len(inputs) == 22 + 5
    # bytearray and lone surrogates are explicitly reference-only.
    assert test_bytes.ByteArrayTest("test_ord").run().wasSuccessful()
    assert all(ord(chr(n)) == n for n in range(0xd800, 0xe000))

    def replay(module, klass, values, answers):
        pairs = iter(zip(values, answers, strict=True))
        def lookup(value):
            native_input, answer = next(pairs)
            assert type(native_input) is type(value) and native_input == value
            if answer.startswith("T"):
                raise TypeError(answer[1:])
            assert answer.startswith("I")
            return int(answer[1:])
        result = original(module, klass, lookup)
        if result.wasSuccessful():
            assert next(pairs, None) is None
        return result

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        source, binary = root/"ord.py", root/"ord"
        source.write_text(
            "def check(lo: int, hi: int) -> int:\n"
            "    for n in range(lo, hi):\n        assert ord(chr(n)) == n\n"
            "    return hi-lo\n\n"
            + "".join(f"try: print('I' + str(ord({value!r})))\nexcept TypeError as e: print('T' + str(e))\n"
                      for value in inputs)
            + "print(check(0, 0xd800) + check(0xe000, 0x110000))\n")
        # Compile the original bytes method body as source too. Its typed fixture
        # supplies type2test and the equality assertion; the slice and loop are original.
        byte_source = root/"bytes_case.py"
        byte_source.write_text("class Case:\n"
            "    def type2test(self, value: bytes) -> bytes:\n        return value\n"
            "    def assertEqual(self, got: list[int], expected: list[int]) -> None:\n"
            "        assert len(got) == len(expected)\n        for i in range(len(got)):\n"
            "            assert got[i] == expected[i]\n"
            + inspect.getsource(test_bytes.BytesTest.test_ord) + "\nCase().test_ord()\n")
        for mode in ([], ["--reachable-only"]):
            for level in ("0", "3"):
                subprocess.run([args.compiler, "build", source, "--no-cache", "--runtime", args.runtime,
                                "--clang", args.clang, "--opt-level", level, "-o", binary, *mode],
                               check=True, capture_output=True)
                output = subprocess.run([binary], check=True, capture_output=True, text=True)
                assert not output.stderr
                values = output.stdout.splitlines()
                assert int(values.pop()) == 1_112_064 and values == expected
                for (module, klass, _), part in zip(cases, [slice(0,22), slice(22,None)], strict=True):
                    subset = values[part]
                    assert replay(module, klass, inputs[part], subset).wasSuccessful()
                    assert replay(module, klass, inputs[part], ["I9999", *subset[1:]]).failures
                subprocess.run([args.compiler, "build", byte_source, "--no-cache", "--runtime", args.runtime,
                                "--clang", args.clang, "--opt-level", level, "-o", binary, *mode],
                               check=True, capture_output=True)
                native = subprocess.run([binary], check=True, capture_output=True)
                assert not native.stdout and not native.stderr
                print(f"{'reachable' if mode else 'whole'} O{level}: original 22 builtin + 5 bytes results; "
                      "one original bytes method native; 1,112,064 native scalars; "
                      "bytearray and 2,048 surrogates on CPython")


if __name__ == "__main__":
    main()
