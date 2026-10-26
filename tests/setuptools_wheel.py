"""Build and consume an unmodified, declarative setuptools extension project."""

import argparse
import os
import shutil
import subprocess
import sys
import sysconfig
import tempfile
from email.parser import BytesParser
from pathlib import Path

from packaging.tags import sys_tags
from packaging.utils import parse_wheel_filename
from wheel.wheelfile import WheelFile

FIXTURES = Path(__file__).resolve().parent / "fixtures"


def make_project(source):
    package = source / "retained_values"
    headers = source / "headers"
    native = source / "native"
    for directory in (package, headers, native):
        directory.mkdir(parents=True)
    shutil.copy(FIXTURES / "setuptools/pyproject.toml", source)
    shutil.copy(FIXTURES / "setuptools/extconfig.py", source)
    shutil.copy(FIXTURES / "setuptools/module.c", native)
    shutil.copy(FIXTURES / "extensions/retainer.pyi", package)
    (package / "__init__.py").touch()
    (package / "format.txt").write_text("format-v2\n")
    (headers / "build_config.h").write_text("#define EXPECTED_VALUE 17\n")
    # Link the second source to the initializer in module.c.
    (native / "retainer.c").write_bytes(
        b"#define PyInit_retainer retainer_init\n"
        + (FIXTURES / "extensions/retainer.c").read_bytes()
    )


def check_wheel(path):
    name, version, _, tags = parse_wheel_filename(path.name)
    assert (name, str(version)) == ("retained-values", "2.4.1")
    (tag,) = tags
    assert (tag.interpreter, tag.abi) == ("imre314", "imre1")
    assert tag.platform in {t.platform for t in sys_tags()}
    with WheelFile(path) as wheel:
        names = wheel.namelist()
        assert not any(name.endswith((".so", ".pyd", ".dylib")) for name in names)
        # WheelFile verifies each member's digest against RECORD on read.
        for name in names:
            wheel.read(name)
        assert wheel.read("retained_values/retainer.imre.a").startswith(b"!<arch>\n")
        assert (
            wheel.read("retained_values/retainer.pyi")
            == (FIXTURES / "extensions/retainer.pyi").read_bytes()
        )
        assert wheel.read("retained_values/format.txt") == b"format-v2\n"
        info = "retained_values-2.4.1.dist-info"
        metadata = BytesParser().parsebytes(wheel.read(f"{info}/METADATA"))
        assert metadata["Summary"] == "Retain a list and call into the application"
        assert metadata["Requires-Python"] == ">=3.11"
        assert metadata.get_all("Requires-Dist") == ["packaging>=23"]
        wheel_info = BytesParser().parsebytes(wheel.read(f"{info}/WHEEL"))
        assert wheel_info["Generator"] == "retained-values"
        assert wheel_info.get_all("Tag") == [str(tag)]
        assert wheel_info["Root-Is-Purelib"] == "false"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()
    compiler = args.compiler.resolve()
    runtime = args.runtime.resolve()

    with tempfile.TemporaryDirectory(prefix="imre-setuptools-") as tmp:
        root = Path(tmp)
        source = root / "existing project"
        site = root / "site"
        make_project(source)
        inputs = {p: p.read_bytes() for p in source.rglob("*") if p.is_file()}
        env = {
            **os.environ,
            "CC": args.clang,
            # Host build flags must not turn the Imre wheel back into a CPython extension.
            "CFLAGS": f'-I"{sysconfig.get_path("include")}"',
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "PYTHONSAFEPATH": "1",
        }

        def run(*command, expected_failure=None):
            result = subprocess.run(
                command,
                cwd=root,
                env=env,
                capture_output=True,
                text=True,
                timeout=120,
                check=False,
            )
            output = result.stdout + result.stderr
            if expected_failure is None:
                assert result.returncode == 0, (command, output)
            else:
                assert result.returncode and expected_failure in output, (
                    command,
                    output,
                )
            return result.stdout

        # Build first for CPython so the input tree contains real, stale host objects.
        run(
            sys.executable,
            "-m",
            "pip",
            "wheel",
            "--no-index",
            "--no-deps",
            "--no-build-isolation",
            "--wheel-dir",
            root / "host",
            source,
        )
        assert list((source / "build").rglob("*.so"))
        target = (
            compiler,
            "wheel",
            source,
            "--python",
            sys.executable,
            "--no-build-isolation",
        )
        run(*target)
        (wheel,) = (source / "dist").glob("*.whl")
        check_wheel(wheel)
        assert all(p.read_bytes() == contents for p, contents in inputs.items())

        install = (
            sys.executable,
            "-m",
            "pip",
            "install",
            "--no-index",
            "--no-deps",
            "--only-binary=:all:",
            "--implementation",
            "imre",
            "--abi",
            "imre1",
            "--python-version",
            "3.14",
            "--target",
            site,
        )
        run(*install, wheel)
        consumer = root / "consumer"
        consumer.mkdir()
        app = consumer / "main.py"
        app.write_text(
            (FIXTURES / "extensions/retained.py")
            .read_text()
            .replace("import retainer", "from retained_values import retainer", 1)
        )
        executable = consumer / "main"
        run(
            compiler,
            "build",
            app,
            "--extension-path",
            site,
            "--runtime",
            runtime,
            "--clang",
            args.clang,
            "-o",
            executable,
        )
        assert run(executable) == "7 8\ncallback failed\nno retained list\n19\n"

        config = source / "pyproject.toml"
        text = config.read_text()
        config.write_text(
            text.replace('requires-python = ">=3.11"', 'requires-python = ">=3.15"')
        )
        incompatible = root / "incompatible"
        run(*target, "--out-dir", incompatible)
        (newer_python,) = incompatible.glob("*.whl")
        run(*install, newer_python, expected_failure="requires a different Python")

        # Missing declarations must not be recovered from CPython's old build directory.
        (source / "retained_values/retainer.pyi").unlink()
        for name, project, diagnostic in (
            ("missing-stub", text, "retainer.pyi"),
            ("pure", text.split("[[tool.setuptools.ext-modules]]")[0], "extension"),
            (
                "backend",
                text.replace("setuptools.build_meta", "another.backend"),
                "backend",
            ),
        ):
            config.write_text(project)
            output = root / name
            run(*target, "--out-dir", output, expected_failure=diagnostic)
            assert not list(output.glob("*.whl"))
        print(
            "Verified setuptools metadata, native build options, and a retained callback."
        )


if __name__ == "__main__":
    main()
