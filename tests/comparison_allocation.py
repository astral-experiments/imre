"""Check both aggregate equality paths while refusing their real scratch allocation."""
import os
from pathlib import Path
import subprocess


def check(args, directory):
    fixtures = Path(__file__).resolve().parent / "fixtures/comparison_allocation"
    source = fixtures / "comparison.py"
    wrapper = directory / "refuse.o"
    subprocess.run([args.clang, "-c", fixtures / "refuse.c", "-o", wrapper], check=True)
    symbols = ("malloc", "realloc", "imre_record_equal", "imre_system_exit_code_equal")
    link = ["-C", f"link-arg={wrapper}"]
    for name in symbols:
        link += ["-C", f"link-arg=-Wl,--wrap={name}"]
    if args.lto_linker is not None:
        link += ["--lto-linker", args.lto_linker.resolve()]
    clean = os.environ.copy()
    clean.pop("IMRE_REFUSE_COMPARISON", None)
    modes = ("direct", "boxed", "scalar")
    expected = {mode: subprocess.check_output([args.python, "-I", source, mode, "3"])
                for mode in modes}
    oom = {mode: subprocess.check_output([args.python, "-I", fixtures / "oracle.py", mode])
           for mode in ("direct", "boxed")}
    passed = 0
    for level in args.opt_levels:
        exe = directory / f"comparison-O{level}"
        subprocess.run([args.compiler.resolve(), "build", source, "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang,
                        "--opt-level", level, "-o", exe, *link], check=True)
        for mode in modes:
            for inject in (False, True):
                env = clean | {"IMRE_REFUSE_COMPARISON": "1"} if inject else clean
                got = subprocess.run([exe, mode, "3"], env=env, capture_output=True)
                failed = inject and mode != "scalar"
                want = (0, oom[mode] if failed else expected[mode],
                        b"refused comparison allocation\n" if failed else b"")
                assert (got.returncode, got.stdout, got.stderr) == want, (level, mode, inject, got, want)
                passed += 1
    return passed
