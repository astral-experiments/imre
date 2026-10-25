"""Force a native dictionary copy to return a pending MemoryError on Linux."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

SOURCE = """import copy
source = {"value": [7]}
try:
    result = copy.copy(source)
    print(len(result))
except MemoryError:
    print("caught")
print(source["value"][0])
"""
# GNU ld replaces only the copy operation. This is the same error ABI a failed
# native table reservation returns: a pending MemoryError and no result owner.
WRAPPER = """
#include <stdint.h>
extern void *imre_exception_new(uint32_t, const void *);
extern void imre_exception_raise(const void *);
extern void imre_exception_release(const void *);
void *__wrap_imre_dict_copy(const void *source) {
    (void)source;
    void *error = imre_exception_new(15, 0);
    imre_exception_raise(error);
    imre_exception_release(error);
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
        source.write_text(SOURCE)
        wrapper.write_text(WRAPPER)
        subprocess.run([args.clang, "-c", wrapper, "-o", obj], check=True)
        oracle = subprocess.run([sys.executable, "-c", "import runpy, sys; from unittest.mock import patch; "
                                 "p=patch('copy.copy',side_effect=MemoryError); p.start(); runpy.run_path(sys.argv[1])",
                                 source], capture_output=True, check=True)
        for level in ("0", "3"):
            subprocess.run([args.imre.resolve(), "build", source, "-o", binary,
                            "--runtime", args.runtime.resolve(), "--clang", args.clang,
                            "--no-cache", "--opt-level", level, "-q",
                            "-C", f"link-arg={obj}", "-C", "link-arg=-Wl,--wrap=imre_dict_copy"], check=True)
            actual = subprocess.run([binary], capture_output=True)
            expected = (oracle.returncode, oracle.stdout, oracle.stderr)
            if (actual.returncode, actual.stdout, actual.stderr) != expected:
                raise AssertionError((level, actual.returncode, actual.stdout, actual.stderr, expected))
            print(f"O{level}: pending copy MemoryError caught before result access; source survives")


if __name__ == "__main__":
    main()
