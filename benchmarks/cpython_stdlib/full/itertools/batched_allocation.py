"""Prove that a failed native batch allocation leaves its input untouched."""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile


SETUP = """from itertools import batched, count
source = count(0)
grouped = batched(source, 257)
"""
BODY = """try:
    next(grouped)
except MemoryError:
    print(next(source))
print(next(grouped)[0])
"""
ORACLE = """from _testcapi import set_nomemory, remove_mem_hooks
try:
    set_nomemory(0, 1)
    next(grouped)
except MemoryError:
    remove_mem_hooks()
    print(next(source))
finally:
    remove_mem_hooks()
print(next(grouped)[0])
"""
# Arm after the constructor returns. Only the requested tuple payload (two
# header words and 257 fields) fails; the iterator, source and exception use
# their real allocations. The retry must return the next complete native tuple.
WRAPPER = """#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
static int armed;
extern void *__real_imre_batched(uint32_t, uintptr_t, uintptr_t, uint32_t);
void *__wrap_imre_batched(uint32_t mode, uintptr_t src, uintptr_t size, uint32_t strict) {
    void *group = __real_imre_batched(mode, src, size, strict);
    armed = 1;
    return group;
}
extern void *__real_malloc(size_t);
void *__wrap_malloc(size_t n) {
    if (armed && n == (2 + 257) * sizeof(uintptr_t)) {
        armed = 0;
        errno = ENOMEM;
        return NULL;
    }
    return __real_malloc(n);
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="imre-batched-allocation-") as tmp:
        root = Path(tmp)
        native, c, obj, exe = (root / n for n in ("batch.py", "wrap.c", "wrap.o", "batch"))
        native.write_text(SETUP + BODY)
        c.write_text(WRAPPER)
        subprocess.run([args.clang, "-c", c, "-o", obj], check=True)
        reference = subprocess.run([sys.executable, "-B", "-c", SETUP + ORACLE], capture_output=True, check=True)
        if (reference.stdout, reference.stderr) != (b"0\n1\n", b""):
            raise AssertionError(f"unexpected CPython allocation control: {reference!r}")
        for level in ("0", "3"):
            subprocess.run([args.imre.resolve(), "build", native, "-o", exe,
                            "--runtime", args.runtime.resolve(), "--clang", args.clang, "--no-cache",
                            "--opt-level", level, "-q", "-C", f"link-arg={obj}",
                            "-C", "link-arg=-Wl,--wrap=imre_batched", "-C", "link-arg=-Wl,--wrap=malloc"], check=True)
            actual = subprocess.run([exe], capture_output=True, check=True)
            if (actual.stdout, actual.stderr) != (reference.stdout, reference.stderr):
                raise AssertionError((level, actual.stdout, actual.stderr))
            print(f"O{level}: tuple allocation failed before the first pull; the same batch cursor retried")


if __name__ == "__main__":
    main()
