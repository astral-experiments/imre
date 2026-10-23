"""Count fresh and reused Relar records on Linux using GNU linker wraps.

Run the same probe with each compiler and the same runtime archive:
    python3 benchmarks/record_reuse.py /path/to/baseline/relar /path/to/librelar_runtime.a
    python3 benchmarks/record_reuse.py /path/to/candidate/relar /path/to/librelar_runtime.a

The shared workload preserves a reference to every prior pair. The cached-result
runtime call allocates a fresh record when it returns a different payload address.
"""

import argparse
from pathlib import Path
import subprocess
import sys
import tempfile


SOURCE = """\
from dataclasses import dataclass
from sys import argv

@dataclass
class Pair:
    left: int
    right: int

def rotate(count: int, keep: bool) -> int:
    pair = Pair(1, 2)
    saved: list[Pair] = []
    for i in range(count):
        if keep:
            saved.append(pair)
        pair = Pair(pair.right, pair.left)
    if keep:
        return saved[0].left + saved[-1].left + pair.left
    return pair.left

print(rotate(10000, argv[1] == "shared"))
"""

COUNTER = r"""
#include <stdio.h>

extern void *__real_relar_record_new(const void *);
extern void *__real_relar_record_cached_result(const void *);
static unsigned fresh, reused, in_cached_result;

void *__wrap_relar_record_new(const void *descriptor) {
    if (!in_cached_result) fresh++;
    return __real_relar_record_new(descriptor);
}

void *__wrap_relar_record_cached_result(const void *old) {
    in_cached_result++;
    void *result = __real_relar_record_cached_result(old);
    in_cached_result--;
    if (result == old) reused++;
    else fresh++;
    return result;
}

__attribute__((destructor)) static void report(void) {
    fprintf(stderr, "fresh=%u reused=%u\n", fresh, reused);
}
"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("compiler", type=Path)
    parser.add_argument("runtime", type=Path)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--source", type=Path, help="run a standalone Python workload")
    parser.add_argument("--reference", type=Path, help="CPython oracle for --source")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("the record counters require a Linux linker with --wrap")
    if args.reference and not args.source:
        parser.error("--reference requires --source")
    with tempfile.TemporaryDirectory(prefix="relar-record-reuse-") as temp:
        root = Path(temp)
        counter, obj, binary = root / "count.c", root / "count.o", root / "probe"
        source = args.source or root / "rotate.py"
        if not args.source:
            source.write_text(SOURCE)
        counter.write_text(COUNTER)
        subprocess.run([args.clang, "-c", counter, "-o", obj], check=True)
        subprocess.run([
            args.compiler.resolve(), "build", source, "-o", binary,
            "--runtime", args.runtime.resolve(), "--clang", args.clang,
            "--no-cache", "--whole-program", "-q",
            "-C", f"link-arg={obj}", "-C", "link-arg=-Wl,--wrap=relar_record_new",
            "-C", "link-arg=-Wl,--wrap=relar_record_cached_result",
        ], check=True)
        for argv in ([[]] if args.source else [["unique"], ["shared"]]):
            expected = subprocess.check_output([sys.executable, args.reference or source, *argv])
            actual = subprocess.run([binary, *argv], capture_output=True, check=True)
            if actual.stdout != expected:
                raise SystemExit(f"CPython returned {expected!r}, Relar returned {actual.stdout!r}")
            label = source.name if args.source else argv[0]
            print(f"{label}: {actual.stderr.decode().strip()} (matches CPython)")


if __name__ == "__main__":
    main()
