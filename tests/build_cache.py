"""Exercise native cache invalidation, command behavior, publication, and fallback."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shutil
import subprocess
from tempfile import TemporaryDirectory


HIT = ["imre cache hit"]
MISS = ["imre cache miss", "imre cache store"]
BYPASS = ["imre cache bypass"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    assert __debug__, "run without -O"
    compiler = args.compiler.resolve()
    checks = []
    with TemporaryDirectory(prefix="imre cache #$ verification ") as temp:
        root = Path(temp)
        project = root / "src"
        project.mkdir()
        entry = project / "main.py"
        dep = project / "helper.py"
        entry.write_text('from sys import argv\nfrom helper import message\nprint(message())\n'
                         'print(argv[0])\nprint(argv[1])\nassert argv[1] != "fail", "requested failure"\n')
        a = 'def message() -> str:\n    return "alpha"\n'
        b = a.replace("alpha", "bravo")
        dep.write_text(a)
        env = dict(os.environ, XDG_CACHE_HOME=str(root / "xdg"), IMRE_CACHE_TRACE="1")
        env.pop("IMRE_CACHE_DIR", None)
        cache = Path(env["XDG_CACHE_HOME"]) / "imre/v1"

        def cli(mode, *flags, output=None, arg="one", e=env, source=entry, cwd=project):
            cmd = [compiler, mode, "--quiet", source, *flags]
            cmd += ["--", arg] if mode == "run" else ["-o", output or root / "build-output"]
            cp = subprocess.run(cmd, capture_output=True, env=e, cwd=cwd)
            log = cp.stderr.decode().splitlines()
            return cp.returncode, cp.stdout.decode(), [s for s in log if s.startswith("imre cache ")], [s for s in log if not s.startswith(("imre cache ", "imre object "))]

        def ok(call, trace, stdout=None):
            code, out, events, err = call
            assert code == 0 and events == trace and not err, call
            if stdout is not None:
                assert out == stdout, (out, stdout)

        def failed(call, events, contains):
            code, _, trace, err = call
            assert code != 0 and trace == events and contains.lower() in "\n".join(err).lower(), call

        def expected(word, arg="one", src=entry):
            return f"{word}\n{src}\n{arg}\n"

        ok(cli("build"), MISS)
        assert cache.stat().st_mode & 0o777 == 0o700
        record, = cache.glob("*.json")
        ok(cli("run"), HIT, expected("alpha"))
        checks.append("A default build warmed run through XDG_CACHE_HOME")
        ok(cli("run", arg="two", source=Path("main.py")), HIT, expected("alpha", "two", "main.py"))
        other = root / "target"
        ok(cli("build", output=other, cwd=root), HIT)
        other.write_bytes(b"caller-owned replacement")
        ok(cli("run"), HIT, expected("alpha"))
        checks.append("Program arguments, argv[0], working directory and materialized outputs are independent of cache identity")
        # GCC's linker script searches for this unqualified library in the working directory.
        shadow = project / "libgcc_s.so.1"
        system_library = next(Path(path) for path, _ in json.loads(record.read_bytes())["native"]
                              if Path(path).is_absolute() and Path(path).name == shadow.name)
        shadow.symlink_to(system_library)
        ok(cli("build"), MISS)
        ok(cli("build"), HIT)
        # The other working directory has a different file under the same name.
        other_shadow = root / shadow.name
        other_shadow.write_text("invalid linker input\n")
        failed(cli("build", cwd=root), ["imre cache miss"], shadow.name)
        failed(cli("build", "--no-cache", cwd=root), BYPASS, shadow.name)
        other_shadow.unlink()
        shadow.unlink()
        ok(cli("build"), MISS)
        ok(cli("build", cwd=root), HIT)
        checks.append("Adding, changing, or removing a working-directory linker-script input invalidated the executable")
        original = project / "source.py"
        original.write_text("print(__file__)\n")
        alias = project / "alias.py"
        alias.symlink_to(original)
        for spelling in (original, alias, original, alias):
            result = cli("run", source=spelling)
            assert result[0] == 0 and result[1] == str(spelling) + "\n", result
        original.write_text("def identity(value: bytes) -> bytes:\n    return value\nprint(identity(1))\n")
        result = cli("build", source=alias)
        assert result[0] != 0 and any(f"{original.resolve()}:3:" in line for line in result[3]), result
        checks.append("Cached programs preserved the source filename when compiled via a symlink")
        collision = dict(env, IMRE_CACHE_DIR=str(root / "shadow-cache"))
        shadow.symlink_to("placed-program")
        ok(cli("build", output=project / "placed-program", e=collision), ["imre cache miss"])
        failed(cli("build", e=collision), ["imre cache miss"], shadow.name)
        shadow.unlink()
        checks.append("A build output that became a linker-script input was kept out of the executable cache")
        verbose = cli("build", "--verbose-link")
        assert verbose[0] == 0 and verbose[2] == ["imre cache relink", "imre cache store"], verbose
        lld = any("--ld-path=" in line for line in verbose[3])
        plain_miss = ["imre cache miss"]
        unusual = dict(env, IMRE_CACHE_DIR=str(root / r"cache\ directory"))
        ok(cli("run", e=unusual), plain_miss if lld else MISS, expected("alpha"))
        ok(cli("run", e=unusual), plain_miss if lld else HIT, expected("alpha"))
        checks.append("Backslash staging paths preserved program stdout and cache safety with either linker")
        # LLD normalizes backslashes in its depfile and omits text scripts from --trace.
        # The other spelling must exist to expose a stale hit after the real INCLUDE changes.
        definitions = root / r"native\symbols.ld"
        alias = root / "native/symbols.ld"
        alias.parent.mkdir()
        alias.write_text("alias_symbol = 123;\n")
        definitions.write_text("actual_symbol = 123;\n")
        shadow.write_text(f'GROUP ( "{system_library}" )\nINCLUDE "{definitions}"\n')
        custom = dict(env, IMRE_CACHE_DIR=str(root / "script-cache"))
        ok(cli("build", e=custom), plain_miss if lld else MISS)
        definitions.write_text('ASSERT(0, "changed nested script")\n')
        failed(cli("build", e=custom), plain_miss, "changed nested script")
        failed(cli("build", "--no-cache", e=custom), BYPASS, "changed nested script")
        shadow.unlink()
        checks.append("A changed nested script with an existing normalized alias reran the actual linker")
        ok(cli("run", e=dict(env, IMRE_CACHE_TRACE="0")), [], expected("alpha"))
        bad_run = cli("run", arg="fail")
        bad_direct = cli("run", "--no-cache", arg="fail")
        assert bad_run[0] != 0 and bad_run[2] == HIT and bad_direct[2] == BYPASS
        assert (bad_run[0:2], bad_run[3]) == (bad_direct[0:2], bad_direct[3]), (bad_run, bad_direct)
        checks.append("Ordinary hits are quiet and a cached program preserves error status and diagnostics")
        unrelated = project / "unrelated.py"
        unrelated.write_text("invalid python # never imported")
        ok(cli("run"), HIT, expected("alpha"))
        checks.append("An unrelated source file did not invalidate the graph")
        st = dep.stat()
        dep.write_text(b)
        os.utime(dep, ns=(st.st_atime_ns, st.st_mtime_ns))
        ok(cli("run"), MISS, expected("bravo"))
        ok(cli("build"), HIT)
        checks.append("A same-size imported edit with restored mtime missed and run warmed build")
        dep.unlink()
        failed(cli("run"), ["imre cache miss"], "cannot resolve")
        dep.write_text("def broken(:\n")
        bad = cli("run")
        assert bad[0] != 0 and not bad[1] and bad[2] == ["imre cache miss"], bad
        dep.write_text(b)
        ok(cli("run"), HIT, expected("bravo"))
        checks.append("Missing and invalid imports did not execute stale code or poison a successful entry")
        pkg = project / "helper"
        pkg.mkdir()
        (pkg / "__init__.py").write_text(a)
        ok(cli("run"), MISS, expected("alpha"))
        (pkg / "__init__.py").unlink()
        ok(cli("run"), MISS, expected("bravo"))
        (project / "sys.py").write_text("")
        failed(cli("run"), ["imre cache miss"], "shadowing")
        (project / "sys.py").unlink()
        ok(cli("run"), HIT, expected("bravo"))
        checks.append("Adding a preferred package or shadowing sys invalidated an existing hit")
        dep.unlink()
        real1, real2 = project / "real1.py", project / "real2.py"
        real1.write_text(b)
        real2.write_text(a)
        dep.symlink_to(real1)
        ok(cli("run"), MISS, expected("bravo"))
        dep.unlink()
        dep.symlink_to(real2)
        ok(cli("run"), MISS, expected("alpha"))
        hard = root / "alias"
        os.link(real2, hard)
        failed(cli("build", output=hard), [], "distinct paths")
        failed(cli("build", output=entry), [], "distinct paths")
        assert real2.read_text() == a
        checks.append("Import symlink retargeting invalidated the entry and output aliases could not overwrite source files")
        link = root / "linked-output"
        link.symlink_to("target-directory/missing")
        (root / "target-directory").mkdir()
        ok(cli("build", output=link), HIT)
        assert link.is_symlink() and os.access(link.resolve(), os.X_OK)
        checks.append("Materializing a hit preserved a dangling output symlink and wrote its target")
        ok(cli("build", "--opt-level", "0"), MISS)
        ok(cli("run", "--opt-level", "0"), HIT, expected("alpha"))
        ok(cli("run", "--allocator", "system"), HIT, expected("alpha"))
        checks.append("Optimization levels have distinct entries and explicit system allocator matched its default")
        if compiler.with_name("libimre_runtime_jemalloc.a").is_file():
            ok(cli("run", "--allocator", "jemalloc"), MISS, expected("alpha"))
            ok(cli("run", "--allocator", "jemalloc"), HIT, expected("alpha"))
            ok(cli("run"), HIT, expected("alpha"))
            checks.append("System and jemalloc runtimes used distinct reusable executables")
        rt = root / "runtime.a"
        shutil.copy2(compiler.with_name("libimre_runtime.a"), rt)
        ok(cli("build", "--runtime", Path("..") / rt.name), MISS)
        ok(cli("run", "--runtime", rt), HIT, expected("alpha"))
        t = rt.stat()
        os.utime(rt, ns=(t.st_atime_ns, t.st_mtime_ns + 1_000_000_000))
        ok(cli("build", "--runtime", rt), MISS)
        checks.append("Choosing or changing the runtime archive invalidated the recipe")
        script = root / "linker-script.a"
        script.write_text(f'INPUT("{rt}")\n')
        ok(cli("run", "--runtime", script), BYPASS, expected("alpha"))
        checks.append("A runtime linker script with indirect inputs bypassed the cache")
        ok(cli("run", "--no-cache"), BYPASS, expected("alpha"))
        ir = root / "out.ll"
        ok(cli("build", "--emit-llvm", ir), BYPASS)
        assert "define " in ir.read_text()
        ok(cli("build", "--clang", shutil.which("clang-18")), BYPASS)
        unavailable = root / "regular-file"
        unavailable.touch()
        ok(cli("run", e=dict(env, IMRE_CACHE_DIR=str(unavailable))), BYPASS, expected("alpha"))
        checks.append("An opt-out, emitted IR, a custom Clang, and an unavailable cache all built successfully without persistent reuse")
        obj = cache / json.loads(record.read_bytes())["executable"]
        obj.write_bytes(b"corrupt executable")
        ok(cli("run"), MISS, expected("alpha"))
        record.write_text("{")
        ok(cli("run"), MISS, expected("alpha"))
        ok(cli("run"), HIT, expected("alpha"))
        checks.append("Corrupt executable and manifest entries were rebuilt and repaired")
        cold = dict(env, IMRE_CACHE_DIR=str(root / "parallel"))
        def cold_build(i):
            return cli("build", output=root / f"race-{i}", e=cold)
        with ThreadPoolExecutor(max_workers=2) as pool:
            completions = list(pool.map(cold_build, (0, 1)))
        for code, out, events, err in completions:
            assert code == 0 and not out and not err and events in (HIT, MISS), completions
        for i in range(2):
            out = subprocess.check_output([root / f"race-{i}", "two"], text=True)
            assert out == expected("alpha", "two", root / f"race-{i}"), out
        ok(cli("run", e=cold), HIT, expected("alpha"))
        checks.append("Two concurrent cold builds published a complete reusable executable")
    if args.output:
        args.output.write_text(json.dumps({"passed": len(checks), "checks": checks}, indent=2) + "\n")
    print(f"{len(checks)} cache behavior groups passed")


if __name__ == "__main__":
    main()
