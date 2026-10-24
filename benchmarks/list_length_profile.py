"""Count native list length calls with GNU ld's --wrap on Linux.

Supply both compiler binaries and use the same runtime archive for both builds.
"""

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


COUNTER = """
#include <stdint.h>
#include <stdio.h>
extern uint64_t __real_relar_list_len(const void *);
static unsigned calls;
uint64_t __wrap_relar_list_len(const void *list) {
    ++calls;
    return __real_relar_list_len(list);
}
__attribute__((destructor)) static void report(void) {
    fprintf(stderr, "list_len_calls=%u\\n", calls);
}
"""

DRIVER = """
def main() -> None:
    xs: list[int] = []
    for i in range(256):
        xs.append(i)
    if argv[1] == "private":
        result = private(xs)
        print(result[-1])
    else:
        ys: list[int] = []
        if argv[1] == "alias":
            ys = xs
        visits, total = scan(xs, ys)
        print(f"{visits} {total}")

main()
"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang-18")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("the length counter requires a Linux linker with --wrap")
    fixtures = Path(__file__).resolve().parents[1] / "tests/fixtures/containers"
    scan = (fixtures / "guarded_list_length.py").read_text().split("def main()", 1)[0]
    private = (fixtures / "fresh_list_length.py").read_text().split("def maybe_alias(", 1)[0]
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        source = directory / "scan.py"
        source.write_text("from sys import argv\n\n" + scan + private + DRIVER)
        counter = directory / "count.c"
        counter.write_text(COUNTER)
        obj = directory / "count.o"
        subprocess.run([args.clang, "-c", counter, "-o", obj], check=True)
        for name, compiler in (("baseline", args.baseline), ("candidate", args.candidate)):
            binary = directory / name
            subprocess.run([
                compiler.resolve(), "build", source, "--runtime", args.runtime.resolve(),
                "--clang", args.clang, "--whole-program", "--no-cache", "-q",
                "-C", f"link-arg={obj}", "-C", "link-arg=-Wl,--wrap=relar_list_len",
                "-o", binary,
            ], check=True)
            for mode in ("distinct", "alias", "private"):
                expected = subprocess.check_output([sys.executable, source, mode])
                actual = subprocess.run([binary, mode], check=True, capture_output=True)
                assert actual.stdout == expected
                print(name, mode, actual.stdout.decode().strip(), actual.stderr.decode().strip())


if __name__ == "__main__":
    main()
