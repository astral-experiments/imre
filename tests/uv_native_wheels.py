"""Build and consume setuptools and scikit-build-core wheels with stock uv."""

import argparse
import json
import os
import shutil
import subprocess
import sys
import sysconfig
from email.parser import BytesParser
from pathlib import Path

from packaging.utils import parse_wheel_filename
from wheel.wheelfile import WheelFile

ROOT = Path(__file__).resolve().parents[1]


def build_wheel(project, dest, *settings, env, expected_error=None):
    command = ["uv", "build", "--wheel", "--python", sys.executable, "--out-dir", dest]
    command += [*settings, project]
    if expected_error:
        result = subprocess.run(
            command, env=env, capture_output=True, text=True, check=False
        )
        assert result.returncode and expected_error in result.stderr, result.stderr
        assert not list(dest.glob("*.whl"))
        return None
    subprocess.run(command, env=env, check=True)
    (wheel,) = dest.glob("*.whl")
    return wheel


def install(wheel, site, *, target=None, module=None):
    args = [sys.executable, "-m", "pip", "install", "--no-index", "--no-deps"]
    args += ["--no-compile", "--only-binary=:all:", "--target", site]
    if target:
        _, _, _, tags = parse_wheel_filename(wheel.name)
        assert {str(tag) for tag in tags} == {target["wheel_tag"]}
        with WheelFile(wheel) as archive:
            (info,) = [
                name for name in archive.namelist() if name.endswith(".dist-info/WHEEL")
            ]
            metadata = BytesParser().parsebytes(archive.read(info))
            assert metadata.get_all("Tag") == [target["wheel_tag"]]
            assert metadata["Root-Is-Purelib"] == "false"
            assert archive.read(module + target["archive_suffix"]).startswith(
                b"!<arch>\n"
            )
            assert archive.read(module + ".pyi")
            assert not any(
                n.endswith((".so", ".pyd", ".dylib")) for n in archive.namelist()
            )
        args += [
            "--implementation",
            target["implementation"],
            "--python-version",
            target["python_version"],
            "--abi",
            target["abi_tag"],
        ]
    subprocess.run([*args, wheel], check=True)


def compile_app(compiler, source, site):
    executable = source.with_suffix("")
    command = [compiler, "build", source, "--extension-path", site]
    subprocess.run(
        [*command, "--clang", os.environ.get("CC", "clang"), "-o", executable],
        check=True,
    )
    return executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument(
        "--upstream", type=Path, required=True, help="google-crc32c 1.8.0 checkout"
    )
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    work = args.work_dir.resolve()
    compiler = args.compiler.resolve()
    sdk = work / "sdk"
    reported = json.loads(subprocess.check_output([compiler, "build-target"]))
    exported = subprocess.check_output([compiler, "build-target", "--output-dir", sdk])
    target_file = sdk / "target.json"
    assert target_file.read_bytes() == exported
    target = json.loads(exported)
    assert all(target[key] == value for key, value in reported.items())
    assert (target["python_tag"], target["abi_tag"]) == ("imre314", "imre1")
    assert target["wheel_tag"] == "imre314-imre1-" + target["platform_tag"]
    assert target["link_mode"] == "static"
    assert (
        Path(target["include_dir"]) / "Python.h"
    ).read_bytes() == subprocess.check_output([compiler, "extension-header"])

    upstream = args.upstream.resolve()
    cxx = "c++" if sys.platform == "darwin" else "stdc++"
    env = {k: v for k, v in os.environ.items() if k != "IMRE_BUILD_TARGET"}
    adapter = Path(target["setuptools_adapter"]).as_uri()
    config = work / "uv.toml"
    config.write_text(
        f'[extra-build-dependencies]\ngoogle-crc32c = ["imre-setuptools @ {adapter}"]\n'
    )
    # Install the plugin in each isolated build. Only the second build opts in.
    cp = build_wheel(
        upstream,
        work / "cpython-dist",
        "--config-file",
        config,
        env={**env, "LDFLAGS": f"-l{cxx}"},
    )
    install(cp, work / "cpython")
    imre_env = {**env, "IMRE_BUILD_TARGET": str(target_file)}
    wheel = build_wheel(
        upstream,
        work / "dist",
        "--config-file",
        config,
        env={
            **imre_env,
            "CFLAGS": f'-I"{sysconfig.get_path("include")}" ' + env.get("CFLAGS", ""),
            "IMRE_WHEEL_OVERLAY": str(ROOT / "examples/google_crc32c/overlay"),
            "IMRE_SYSTEM_LIBRARIES": cxx,
        },
    )
    install(wheel, work / "site", target=target, module="google_crc32c/_crc32c")

    consumer = work / "consumer"
    consumer.mkdir()
    checksum = consumer / "driver.py"
    shutil.copyfile(ROOT / "examples/google_crc32c/driver.py", checksum)
    driver = compile_app(compiler, checksum, work / "site")
    assert subprocess.check_output([driver, "public"]) == b"public-api-ok\n"

    fixture = ROOT / "tests/fixtures/scikit_build"
    cp = build_wheel(fixture, work / "scikit-cpython-dist", env=env)
    cp_site = work / "scikit-cpython"
    install(cp, cp_site)
    scikit_settings = (
        "-Cwheel.tags=" + target["wheel_tag"],
        "-Ccmake.define.CMAKE_PROJECT_TOP_LEVEL_INCLUDES="
        + target["cmake_project_include"],
    )
    native = build_wheel(fixture, work / "scikit-dist", *scikit_settings, env=imre_env)
    site = work / "scikit-site"
    install(native, site, target=target, module="bitcount/_native")
    script = consumer / "count.py"
    script.write_text("from bitcount import popcount\nprint(popcount(b'\\x0f\\xff'))\n")
    cp_result = subprocess.check_output(
        [sys.executable, "-B", script], env={**env, "PYTHONPATH": str(cp_site)}
    )
    binary = compile_app(compiler, script, site)
    assert subprocess.check_output([binary]) == cp_result == b"12\n"

    # This project bypasses Imre's FindPython.cmake and produces a host shared object.
    unsupported = work / "python3-extension"
    shutil.copytree(fixture, unsupported)
    cmake = unsupported / "CMakeLists.txt"
    cmake.write_text(cmake.read_text().replace("Python", "Python3"))
    build_wheel(
        unsupported,
        work / "invalid-dist",
        *scikit_settings,
        env=imre_env,
        expected_error="unexpected shared library in Imre wheel",
    )


if __name__ == "__main__":
    main()
