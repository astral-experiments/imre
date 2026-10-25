"""Exercise real cursors, including pulls after errors, against a Python executable."""
import argparse
from hashlib import sha256
from itertools import product
from pathlib import Path
import random
import subprocess
import sys
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent


def corpus():
    fixed = ["", "{} bad } tail {okay}", "a.b[]bad.z[٠٢]", "001.ᾀ[٠٢]", "99" * 15 + ".a",
             "x[99999999999999999999q].okay", "x[00000000000000000000000000000003]",
             "x[٠١٢٣٤٥٦٧٨٩]", ".", "[-1]", "[²]", "[９９８]", "{0!\0:hello}", "{0!💧:α{ɐ}}",
             "abc{", "{}x{", "x[𐐀].x", "x{0}tail", "{{}}", "a[\0]"]
    short = ["".join(row) for n in range(5) for row in product("{}[]!", repeat=n)]
    rng = random.Random(5607950)
    mixed = ["".join(rng.choices("{}!:.[]01abc𐐀Ɂ٠٩\0", k=rng.randrange(1, 70))) for _ in range(10000)]
    tokens = ["x", "ß", "𐐀", "٢", "０", "99999999999999999999", "\0", "{", "}", "!", ":",
              "[", "]", ".", "[]", ".attr", "[word]", "[1]", "{0}", "{val!s}", "{{", "}}"]
    combinations = ["".join(rng.choices(tokens, k=rng.randrange(1, 25))) for _ in range(3500)]
    bounds = [s for n in (sys.maxsize - 1, sys.maxsize, sys.maxsize + 1)
              for s in (str(n), str(n) + ".ok", "first[0][" + str(n) + "].last")]
    return list(dict.fromkeys(fixed + short + mixed + combinations + bounds))


def output(argv):
    result = subprocess.run(argv, capture_output=True, check=True)
    if result.stderr:
        sys.stderr.buffer.write(result.stderr)
        raise SystemExit("unexpected stderr from " + str(argv[0]))
    return result.stdout


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--compiler", required=True, type=Path)
    p.add_argument("--runtime", required=True, type=Path)
    p.add_argument("--python", default=sys.executable)
    p.add_argument("--level", choices=["0", "3"], required=True)
    p.add_argument("--reachable-only", action="store_true")
    p.add_argument("--sanitize", action="store_true")
    p.add_argument("--originals", action="store_true",
                   help="use a Python containing the pinned original unittest files")
    args = p.parse_args()
    with TemporaryDirectory() as temp:
        root = Path(temp)
        inputs = root / "inputs.txt"
        values = corpus()
        inputs.write_bytes(("\n".join(values) + "\n").encode())
        print(f"{len(values)} inputs plus two embedded newline controls")
        for name in ("cursor_probe", "call_errors", "ownership"):
            source = (HERE / "cursor_probe.py" if name == "cursor_probe" else
                      HERE.parents[3] / "tests" / "fixtures" / "text" / f"formatter_{name}.py")
            exe = root / name
            command = [args.compiler, "build", source, "-o", exe, "--runtime", args.runtime, "--opt-level", args.level]
            if args.reachable_only:
                command.append("--reachable-only")
            if args.sanitize:
                command.append("--sanitize")
            subprocess.run(command, check=True)
            arguments = [inputs] if name == "cursor_probe" else []
            expected, actual = output([args.python, source, *arguments]), output([exe, *arguments])
            if actual != expected:
                for index, (got, want) in enumerate(zip(actual.splitlines(), expected.splitlines())):
                    if got != want:
                        raise AssertionError((name, index + 1, got, want))
                raise AssertionError((name, "different lengths", len(actual), len(expected)))
            print(f"{name}: {len(actual.splitlines())} lines, {sha256(actual).hexdigest()}")
            if args.originals and name == "cursor_probe":
                stock = root / "stock_parse.py"
                subprocess.run([args.python, HERE / "originals.py", exe, "--emit-parse", stock], check=True)
                stock_exe = root / "stock_parse"
                stock_build = [stock if part == source else stock_exe if part == exe else part for part in command]
                subprocess.run(stock_build, check=True)
                assert output([stock_exe]) == output([args.python, stock]), "unchanged public Formatter.parse"


if __name__ == "__main__":
    main()
