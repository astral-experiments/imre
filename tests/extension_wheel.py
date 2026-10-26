"""Build the same retained-list extension for CPython and a standalone Imre app."""

import argparse
import os
import shutil
import subprocess
import sys
import sysconfig
import tempfile
from pathlib import Path

from packaging.utils import parse_wheel_filename


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/fixtures/extensions"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    compiler = args.compiler.resolve()
    runtime = args.runtime.resolve()

    with tempfile.TemporaryDirectory(prefix="imre-extension-") as temp:
        root = Path(temp)
        stage, include, cpython, app = [
            root / name for name in ("stage", "include", "cpython", "app")
        ]
        for directory in (stage, include, cpython, app):
            directory.mkdir()
        dist, site = root / "dist", root / "installed"
        env = {
            **os.environ,
            "IMRE_CACHE_DIR": str(root / "cache"),
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
        }
        if args.sanitize:
            env["ASAN_OPTIONS"] = "detect_leaks=1:halt_on_error=1"

        def run(*command: str | Path, success: bool = True) -> bytes:
            result = subprocess.run(
                command, cwd=root, env=env, capture_output=True, timeout=120
            )
            assert (result.returncode == 0) == success, (
                command,
                result.returncode,
                result.stdout.decode(),
                result.stderr.decode(),
            )
            return result.stdout if success else result.stderr

        source = FIXTURES / "retainer.c"
        (include / "Python.h").write_bytes(run(compiler, "extension-header"))
        cflags = ["-std=c11", "-fPIC", "-O2"]
        cp_link = (
            ["-bundle", "-undefined", "dynamic_lookup"]
            if sys.platform == "darwin"
            else ["-shared"]
        )
        cp_module = cpython / ("retainer" + sysconfig.get_config_var("EXT_SUFFIX"))
        run(
            args.clang,
            *cflags,
            *cp_link,
            "-I",
            sysconfig.get_path("include"),
            source,
            "-o",
            cp_module,
        )
        native_flags = ["-fsanitize=address"] if args.sanitize else []
        run(
            args.clang,
            *cflags,
            *native_flags,
            "-I",
            include,
            "-c",
            source,
            "-o",
            root / "retainer.o",
        )
        run("ar", "rcs", stage / "retainer.imre.a", root / "retainer.o")
        shutil.copyfile(FIXTURES / "retainer.pyi", stage / "retainer.pyi")
        run(
            sys.executable,
            ROOT / "scripts/pack_extension_wheel.py",
            stage,
            "--name",
            "retainer",
            "--version",
            "1.0",
            "--output",
            dist,
        )
        (wheel,) = dist.glob("*.whl")
        _, _, _, tags = parse_wheel_filename(wheel.name)
        (tag,) = tags
        assert (tag.interpreter, tag.abi) == ("im314", "imre1")
        pip = (
            sys.executable,
            "-m",
            "pip",
            "install",
            "--no-index",
            "--no-deps",
            "--no-compile",
            "--only-binary=:all:",
            "--target",
            site,
        )
        assert b"not a supported wheel" in run(*pip, wheel, success=False)
        run(
            *pip,
            "--implementation",
            "im",
            "--python-version",
            "3.14",
            "--abi",
            tag.abi,
            wheel,
        )
        assert (site / "retainer.imre.a").read_bytes() == (
            stage / "retainer.imre.a"
        ).read_bytes()

        cp_runner = "import runpy, sys; sys.path.insert(0, sys.argv[1]); runpy.run_path(sys.argv[2], run_name='__main__')"
        programs = {}
        for name in ("retained", "typed_write"):
            path = app / f"{name}.py"
            shutil.copyfile(FIXTURES / path.name, path)
            cp_output = run(sys.executable, "-I", "-B", "-c", cp_runner, cpython, path)
            if name == "retained":
                assert cp_output == b"7 8\ncallback failed\nno retained list\n19\n"
                expected = cp_output
            else:
                assert cp_output == b"stored b'foreign write'\n"
                expected = b"rejected 42\nrejected result\n"
            assert b"retainer" in run(compiler, "check", path, success=False)
            for level in ("0", "3"):
                binary = root / f"{name}-O{level}"
                flags = ["--sanitize"] if args.sanitize else []
                run(
                    compiler,
                    "build",
                    path,
                    "--extension-path",
                    site,
                    "--opt-level",
                    level,
                    "--runtime",
                    runtime,
                    "--clang",
                    args.clang,
                    *flags,
                    "--quiet",
                    "-o",
                    binary,
                )
                programs[binary] = expected

        if not args.sanitize:
            cached = root / "cached"
            build = (
                compiler, "build", app / "retained.py", "--extension-path", site,
                "--runtime", runtime, "--quiet", "-o", cached,
            )
            run(*build)
            run(*build)
            assert run(cached) == programs[root / "retained-O3"]

            # Changing a wheel at the same path must affect the next link.
            manifest = site / "retainer.imre.libs"
            manifest.write_text("--invalid-library\n")
            assert b"invalid native link library name" in run(*build, success=False)
            manifest.unlink()
            archive = site / "retainer.imre.a"
            archive.write_bytes(b"invalid replacement archive\n")
            assert b"LLVM compilation/linking failed" in run(*build, success=False)

        # Neither the wheel's source files nor the archive are needed at execution time.
        site.rename(root / "unavailable")
        for binary, expected in programs.items():
            assert run(binary) == expected
        print(
            "Verified a rebuilt wheel, retained identities, reentrant callbacks, and checked native types at O0 and O3."
        )


if __name__ == "__main__":
    main()
