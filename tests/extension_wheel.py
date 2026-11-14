"""Build the same retained-list extension for CPython and a standalone Imre app."""

import argparse
import os
import shutil
import subprocess
import sys
import sysconfig
import tempfile
import zipfile
from pathlib import Path

import packaging
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

        def run(
            *command: str | Path,
            success: bool = True,
            extra_env: dict[str, str] | None = None,
        ) -> bytes:
            result = subprocess.run(
                command, cwd=root, env=env | (extra_env or {}), capture_output=True, timeout=120
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
            "--imre",
            compiler,
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
        assert (tag.interpreter, tag.abi) == ("imre314", "imre1")
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
            "imre",
            "--python-version",
            "3.14",
            "--abi",
            tag.abi,
            wheel,
        )
        assert (site / "retainer.imre.a").read_bytes() == (
            stage / "retainer.imre.a"
        ).read_bytes()

        venv = root / "venv"
        launcher = venv / "bin" / "python"
        launcher.parent.mkdir(parents=True)
        launcher.symlink_to(compiler)
        (venv / "pyvenv.cfg").write_text(
            f"home = {compiler.parent}\ninclude-system-site-packages = false\n"
        )
        installed = venv / "lib/imre3.14/site-packages"
        installed.mkdir(parents=True)
        caller = app / "installed.py"
        caller.write_text("import retainer\nprint(retainer.buffer_bytes(b'wheel', 0))\n")
        flags = ["--runtime", str(runtime), "--clang", args.clang, "--opt-level", "0"]
        if args.sanitize:
            flags.append("--sanitize")
        startup = {"IMRE_ENCODED_FLAGS": "\x1f".join(flags), "PYTHONPATH": ""}
        if sys.platform == "linux" and sys.version_info[:2] == (3, 14):
            binary = installed / cp_module.name
            shutil.copyfile(cp_module, binary)
            error = run(launcher, "-B", caller, extra_env=startup, success=False)
            assert os.fsencode(binary) in error and b"CPython shared-extension ABI" in error

            package = installed / "retainer"
            package.mkdir()
            initializer = package / ("__init__" + sysconfig.get_config_var("EXT_SUFFIX"))
            shutil.copyfile(cp_module, initializer)
            error = run(launcher, "-B", caller, extra_env=startup, success=False)
            assert os.fsencode(initializer) in error and b"CPython shared-extension ABI" in error

            archive = root / "namespace.zip"
            with zipfile.ZipFile(archive, "w") as zf:
                zf.writestr("retainer/", b"")
            error = run(
                launcher, "-B", caller,
                extra_env=startup | {"PYTHONPATH": str(archive)}, success=False,
            )
            assert os.fsencode(initializer) in error and b"CPython shared-extension ABI" in error

            # The suffix describes the filename, even when the file is empty.
            initializer.write_bytes(b"")
            error = run(launcher, "-B", caller, extra_env=startup, success=False)
            assert os.fsencode(initializer) in error and b"CPython shared-extension ABI tag" in error
            bare = initializer.with_name("__init__.so")
            initializer.rename(bare)
            error = run(launcher, "-B", caller, extra_env=startup, success=False)
            assert os.fsencode(bare) in error and b"untagged shared-library filename" in error
            assert b"CPython" not in error
        (installed / "retainer.py").write_text(
            "def buffer_bytes(data: bytes, mode: int, /) -> bytes: return b'source'\n"
        )
        shutil.copyfile(site / "retainer.pyi", installed / "retainer.pyi")
        assert run(launcher, "-B", caller, extra_env=startup) == b"b'source'\n"
        shutil.copyfile(site / "retainer.imre.a", installed / "retainer.imre.a")
        assert run(launcher, "-B", caller, extra_env=startup) == b"b'wheel'\n"

        shutil.copytree(Path(packaging.__file__).parent, installed / "packaging")
        probe = (
            "from packaging.tags import generic_tags\n"
            "from sysconfig import get_config_var\n"
            f"assert {str(tag)!r} in {{str(t) for t in generic_tags(platforms=[{tag.platform!r}])}}\n"
            "print(get_config_var('EXT_SUFFIX'))"
        )
        suffix = run(launcher, "-B", "-c", probe, extra_env=startup).decode().strip()
        (installed / "retainer.imre.a").rename(installed / ("retainer" + suffix))
        assert run(launcher, "-B", caller, extra_env=startup) == b"b'wheel'\n"

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
