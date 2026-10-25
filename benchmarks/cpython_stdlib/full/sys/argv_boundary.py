"""Check real process arguments in Imre's two launch modes (not upstream IDs)."""

import argparse
import ast
import os
from pathlib import Path
import subprocess
import tempfile


PROGRAM = """\
import sys
from sys import argv, orig_argv

print(argv)
print(orig_argv)
argv.append("script-added")
orig_argv.append("launcher-added")
print(sys.argv[-1])
print(sys.orig_argv[-1])
"""


def output(command):
    result = subprocess.run(command, capture_output=True, text=True, timeout=45)
    if result.returncode:
        raise RuntimeError(f"{command[:2]}: {result.stderr}")
    return result.stdout.splitlines()


def check(command, script_argv):
    argv, original, script_last, original_last = output(command)
    assert ast.literal_eval(argv) == script_argv
    assert ast.literal_eval(original) == command
    assert (script_last, original_last) == ("script-added", "launcher-added")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    compiler = str(args.imre.resolve())
    flags = ["--runtime", str(args.runtime.resolve()), "--clang", args.clang,
             "--opt-level", args.opt_level, "--quiet"]
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        source = str(root / "program.py")
        binary = str(root / "program")
        Path(source).write_text(PROGRAM, encoding="utf-8")
        output([compiler, "build", source, *flags, "-o", binary])
        # On Linux the second vector exceeds the startup socket's buffer.
        # Allow for environments with a smaller exec limit.
        chunks = min(20, os.sysconf("SC_ARG_MAX") // (2 * 32768))
        for values in (["", "café 🦀", "--", "two words"],
                       [str(i) + "a" * 32760 for i in range(chunks)]):
            check([binary, *values], [binary, *values])
            check([compiler, "run", source, *flags, "--", *values], [source, *values])

        # Reading only orig_argv must still request the launcher's vector.
        Path(source).write_text("import sys\nprint(sys.orig_argv)\n", encoding="utf-8")
        command = [compiler, "run", source, *flags, "--", ""]
        assert ast.literal_eval(output(command)[0]) == command
    print(f"sys argv boundaries passed at -O{args.opt_level}")


if __name__ == "__main__":
    main()
