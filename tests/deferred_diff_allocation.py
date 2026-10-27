"""Refuse real allocations after creating two ndiff aliases and two tee cursors."""
import os
from pathlib import Path
import re
import subprocess


def check(args, directory):
    fixtures = Path(__file__).resolve().parent / "fixtures/deferred_diff_allocation"
    wrapper = directory / "deferred-diff-refuse.o"
    subprocess.run([args.clang, "-c", fixtures / "refuse.c", "-o", wrapper], check=True)
    link = ["-C", f"link-arg={wrapper}"]
    for name in ("malloc", "realloc", "calloc", "imre_print_str"):
        link += ["-C", f"link-arg=-Wl,--wrap={name}"]
    if args.lto_linker is not None:
        link += ["--lto-linker", args.lto_linker.resolve()]
    source = fixtures / "retry.py"
    oracle = (
        "import runpy, sys\n"
        f"sys.path.insert(0, {str(fixtures.parents[2] / 'stdlib')!r})\n"
        "sys.argv.pop(0)\n"
        "runpy.run_path(sys.argv[0], run_name='__main__')"
    )
    modes = ("len", "copy", "copy-during", "index", "tee-first", "tee-second", "reverse", "project", "repr", "boxed", "union", "frozen")
    cases = [(m, "short", "A", other) for other in ("A", "B") for m in modes]
    cases.append(("deepcopy-graph", "short", "A", "B"))
    cases += [(mode, "short", "A", "B") for mode in ("extend-list", "extend-self", "extend-converted")]
    cases.append(("join-owner", "short", "A", "B"))
    cases += [(m, "sparse", "A", "B") for m in ("tee-boundary", "tee-cached", "pack-project", "restore")]
    cases.append(("slice-owner", "short", "A", "B"))
    cases += [(mode, shape, "A", "B") for mode in ("packed-slice", "packed-repeat")
              for shape in ("empty", "short")]
    cases += [("dict-copy-owners", shape, "A", "B") for shape in ("empty", "full")]
    passed = 0
    for level in args.opt_levels:
        exe = directory / f"deferred-diff-O{level}"
        subprocess.run([args.compiler.resolve(), "build", source, "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang,
                        "--opt-level", level, "-o", exe, *link], check=True)
        for argv in cases:
            expected = subprocess.check_output([args.python, "-I", "-c", oracle, source, *argv])
            env = os.environ | {"IMRE_DIFF_FAIL_N": "0"}
            control = subprocess.run([exe, *argv], env=env, capture_output=True, check=True)
            assert control.stdout == expected, (level, argv, control)
            count = int(re.fullmatch(rb"observed (\d+)\n", control.stderr)[1])
            assert count > 0, (level, argv, "no allocations observed")
            # repr's output buffer is covered separately; target its first row allocation.
            if argv[0] in ("packed-slice", "packed-repeat"):
                # Packed storage is allocated just before the enclosing list owner.
                indices = (count - 1,)
            elif argv[0] == "slice-owner":
                indices = (count,)
            elif argv[0] in ("repr", "frozen"):
                indices = (1,)
            elif argv[0] == "extend-converted":
                indices = {1, count}
            elif argv[0] in ("extend-list", "extend-self"):
                indices = (count,)
            elif argv[0] == "join-owner":
                indices = (count,)
            elif argv[0] in ("pack-project", "restore"):
                indices = {1, count // 2, count}
            elif argv[0] == "dict-copy-owners":
                # Insertion finishes before the two result owners are allocated.
                indices = (count - 1, count)
            else:
                indices = range(1, count + 1)
            refusal = b"refused\n" if argv[0] == "deepcopy-graph" else b"refused\nMemoryError()\n"
            retry = expected.replace(b"armed\n", b"armed\n" + refusal, 1)
            for at in indices:
                env["IMRE_DIFF_FAIL_N"] = str(at)
                got = subprocess.run([exe, *argv], env=env, capture_output=True)
                assert (got.returncode, got.stdout) == (0, retry), (level, argv, at, got)
                assert re.fullmatch(fr"refused {at} \d+\nobserved {at}\n".encode(), got.stderr), got
                passed += 1
            passed += 1
    return passed
