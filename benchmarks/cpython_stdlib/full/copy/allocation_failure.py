"""Force native shallow and deep copies to report an allocation failure on Linux."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

DICT_SOURCE = """import copy
source = {"value": [7]}
try:
    result = copy.copy(source)
    print(len(result))
except MemoryError:
    print("caught")
print(source["value"][0])
"""
RECORD_SOURCE = """import copy
class Box:
    value: list[int]
    def __init__(self, value: list[int]) -> None:
        self.value = value
source = Box([7])
try:
    result = copy.copy(source)
    print(result.value[0])
except MemoryError:
    print("caught")
print(source.value[0])
"""
# GNU ld replaces only the copy operation. This is the same error ABI a failed
# native allocation returns: a pending MemoryError and no result owner.
WRAPPER = """
#include <stdint.h>
extern void *imre_exception_new(uint32_t, const void *);
extern void imre_exception_raise(const void *);
extern void imre_exception_release(const void *);
static void allocation_failed(void) {
    void *error = imre_exception_new(15, 0);
    imre_exception_raise(error);
    imre_exception_release(error);
}
void *__wrap_imre_dict_copy(const void *source) {
    (void)source;
    allocation_failed();
    return 0;
}
void *__wrap_imre_record_shallow_copy(const void *source) {
    (void)source;
    allocation_failed();
    return 0;
}
uintptr_t __wrap_imre_deep_copy(uintptr_t source, uint32_t kind) {
    (void)source;
    (void)kind;
    allocation_failed();
    return 0;
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        source, wrapper, obj, binary = (work / n for n in ("case.py", "fail.c", "fail.o", "case"))
        wrapper.write_text(WRAPPER)
        subprocess.run([args.clang, "-Wall", "-Werror", "-c", wrapper, "-o", obj], check=True)
        for operation, python_name, program in (
            ("imre_dict_copy", "copy.copy", DICT_SOURCE),
            ("imre_record_shallow_copy", "copy.copy", RECORD_SOURCE),
            ("imre_deep_copy", "copy.deepcopy", DICT_SOURCE.replace("copy.copy(source)", "copy.deepcopy(source)")),
        ):
            source.write_text(program)
            oracle = subprocess.run([sys.executable, "-c", "import runpy, sys; from unittest.mock import patch; "
                                     "p=patch(sys.argv[1],side_effect=MemoryError); p.start(); runpy.run_path(sys.argv[2])",
                                     python_name, source], capture_output=True, check=True)
            for level in ("0", "3"):
                subprocess.run([args.imre.resolve(), "build", source, "-o", binary,
                                "--runtime", args.runtime.resolve(), "--clang", args.clang,
                                "--no-cache", "--opt-level", level, "-q",
                                "-C", f"link-arg={obj}", "-C", f"link-arg=-Wl,--wrap={operation}"], check=True)
                actual = subprocess.run([binary], capture_output=True)
                expected = (oracle.returncode, oracle.stdout, oracle.stderr)
                if (actual.returncode, actual.stdout, actual.stderr) != expected:
                    raise AssertionError((level, actual.returncode, actual.stdout, actual.stderr, expected))
                print(f"O{level}: pending {operation} MemoryError caught before result access; source survives")


if __name__ == "__main__":
    main()
