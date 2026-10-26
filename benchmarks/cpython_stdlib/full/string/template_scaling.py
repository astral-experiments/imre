"""Keep Unicode Template substitution from rescanning every prefix (Linux Callgrind)."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


SOURCE = '''from string import Template
import sys

print(Template("é$word " * int(sys.argv[1])).substitute(word="value"))
'''
SIZES = (0, 256, 2048)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--valgrind", default="valgrind")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    instructions = {}
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        src, exe = work / "dense.py", work / "dense"
        src.write_text(SOURCE)
        subprocess.run([args.imre.resolve(), "build", src, "-o", exe, "--opt-level", "3",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang], check=True)
        for size in SIZES:
            reference = subprocess.check_output([args.python, src, str(size)])
            trace = work / f"{size}.callgrind"
            actual = subprocess.check_output(
                [args.valgrind, "--tool=callgrind", "--vgdb=no", "--quiet", f"--callgrind-out-file={trace}",
                 exe, str(size)])
            assert actual == reference, f"incorrect Unicode substitution at {size} placeholders"
            lines = trace.read_text().splitlines()
            assert "events: Ir" in lines
            totals = [int(line.removeprefix("totals: ")) for line in lines if line.startswith("totals: ")]
            assert len(totals) == 1
            instructions[size] = totals[0]

    empty, small, large = (instructions[n] for n in SIZES)
    assert empty < small < large
    # Subtract module initialization and process startup. An 8x larger template
    # has 8x as many matches: leave 2x slack for allocation and toolchain changes.
    growth = (large - empty) / (small - empty)
    data = {"placeholders": list(SIZES), "instructions": list(instructions.values()),
            "growth_excluding_startup": growth, "maximum_growth": 16}
    if args.report:
        args.report.write_text(json.dumps(data, indent=2) + "\n")
    print(f"8x Unicode placeholders: {growth:.2f}x instructions after startup")
    assert growth <= data["maximum_growth"], "prefix scans make substitution superlinear"


if __name__ == "__main__":
    main()
