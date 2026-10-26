"""Retarget setuptools' native extension commands inside a wheel build."""

import os
import re
import shlex
import shutil
import subprocess
import sys
import sysconfig
from pathlib import Path
from tempfile import TemporaryDirectory

from setuptools.command.build_ext import build_ext

try:
    from setuptools.command.bdist_wheel import bdist_wheel
except ImportError:  # Older setuptools gets this command from wheel.
    from wheel.bdist_wheel import bdist_wheel


def _archive(output, inputs):
    """Flatten static dependencies so the wheel can move off the build machine."""
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=output.parent) as temporary:
        work = Path(temporary)
        archive = work / "extension.a"
        if sys.platform == "darwin":
            subprocess.run(
                ["/usr/bin/libtool", "-static", "-o", archive, *inputs], check=True
            )
        else:
            # GNU and LLVM ar accept MRI scripts. Use local names because paths in
            # these scripts cannot portably contain spaces or other punctuation.
            script = ["CREATE extension.a"]
            for index, path in enumerate(inputs):
                member = f"input{index}"
                (work / member).symlink_to(path.resolve(strict=True))
                script.append(
                    f"{'ADDLIB' if path.suffix == '.a' else 'ADDMOD'} {member}"
                )
            subprocess.run(
                [*shlex.split(os.environ.get("AR", "ar")), "-M"],
                cwd=work,
                input="\n".join([*script, "SAVE", "END", ""]),
                text=True,
                check=True,
            )
        os.replace(archive, output)


def _target_headers(command, include, host_headers):
    # setuptools puts flags from CFLAGS/CPPFLAGS *before* Extension.include_dirs.
    # Retarget C/C++ command arguments, including split -I arguments.
    result = []
    args = iter(command)
    for arg in args:
        option = next(
            (s for s in ("-isystem", "-iquote", "-I") if arg.startswith(s)), None
        )
        if option is None:
            result.append(arg)
            continue
        joined = arg[len(option) :]
        directory = joined or next(args)
        path = Path(directory)
        if path.resolve() in host_headers:
            result.extend((option, str(include)))
        else:
            if path.resolve() != include and (path / "Python.h").is_file():
                result.extend((option, str(include)))
            result.append(arg)
            if not joined:
                result.append(directory)
    return result


def install_target(include_dir, overlay_dir=None, system_libraries=()):
    """Install the Imre target before invoking an upstream setuptools hook."""
    include = Path(include_dir).resolve(strict=True)
    if not (include / "Python.h").is_file():
        raise ValueError(f"missing Imre Python.h in {include}")
    if sys.platform not in {"linux", "darwin"}:
        raise ValueError("Imre native wheels currently require Linux or macOS")
    overlay = Path(overlay_dir).resolve(strict=True) if overlay_dir else None
    system = list(dict.fromkeys(system_libraries))
    if any(not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9._+-]*", lib) for lib in system):
        raise ValueError("invalid system library name")

    upstream_build = build_ext.build_extensions
    upstream_metadata = bdist_wheel.write_wheelfile
    upstream_tag = bdist_wheel.get_tag
    built = set()

    def build_extensions(self):
        # Keep the project's build_ext subclass, macros, flags and language
        # selection. Only the target header and the final link differ.
        compiler = self.compiler
        host_headers = {
            Path(sysconfig.get_path(name)).resolve()
            for name in ("include", "platinclude")
        }

        def target_dirs(dirs):
            return [
                str(include),
                *(p for p in dirs if Path(p).resolve() not in host_headers),
            ]

        compiler.set_include_dirs(target_dirs(compiler.include_dirs))
        for ext in self.extensions:
            ext.include_dirs = target_dirs(ext.include_dirs)
            ext.extra_compile_args = _target_headers(
                ext.extra_compile_args, include, host_headers
            )
        for name in ("compiler_so", "compiler_so_cxx", "compiler_cxx"):
            if flags := getattr(compiler, name, None):
                setattr(compiler, name, _target_headers(flags, include, host_headers))
        self.force = compiler.force = True

        def link(
            objects,
            output_filename,
            *,
            libraries,
            library_dirs,
            runtime_library_dirs,
            extra_postargs,
            target_lang,
            **kwargs,
        ):
            output = Path(output_filename)
            dirs = [Path(p) for p in [*library_dirs, *compiler.library_dirs]]
            link_libraries = [*libraries, *compiler.libraries]
            flags = iter(
                [
                    *shlex.split(os.environ.get("LDFLAGS", "")),
                    *(kwargs.get("extra_preargs") or ()),
                    *extra_postargs,
                ]
            )
            for flag in flags:
                if not flag.startswith(("-L", "-l")):
                    raise ValueError(
                        f"unsupported linker flag for {output.name}: {flag}"
                    )
                value = flag[2:] or next(flags, None)
                if not value:
                    raise ValueError(f"missing argument for linker flag {flag}")
                if flag.startswith("-L"):
                    dirs.append(Path(value))
                else:
                    link_libraries.append(value)
            if any(
                Path(p) not in dirs
                for p in [*runtime_library_dirs, *compiler.runtime_library_dirs]
            ):
                raise ValueError(
                    f"{output.name}: a runtime-only library search path cannot be packaged"
                )
            inputs = [Path(p) for p in [*objects, *compiler.objects]]
            needed = system.copy()
            if target_lang == "c++" and not {"stdc++", "c++"}.intersection(needed):
                needed.append("c++" if sys.platform == "darwin" else "stdc++")
            for lib in link_libraries:
                if lib in needed:
                    continue
                archive = next(
                    (p / f"lib{lib}.a" for p in dirs if (p / f"lib{lib}.a").is_file()),
                    None,
                )
                if archive is None:
                    raise ValueError(
                        f"missing lib{lib}.a for {output.name} in library_dirs "
                        f"(for a system library, pass --system-library {lib})"
                    )
                inputs.append(archive)
            if any(p.suffix not in {".o", ".a"} for p in inputs):
                raise ValueError(
                    f"{output.name}: expected object files or static libraries"
                )
            _archive(output, inputs)
            if needed:
                output.with_suffix(".libs").write_text(
                    "".join(f"{lib}\n" for lib in needed)
                )
            built.add(output.resolve().relative_to(Path(self.build_lib).resolve()))

        compiler.link_shared_object = link
        upstream_build(self)

    def get_tag(self):
        if self.root_is_pure:
            raise ValueError(
                "setuptools selected a pure-Python wheel for an Imre native extension build"
            )
        platform = upstream_tag(self)[2]
        if sys.platform == "darwin":
            from packaging.tags import mac_platforms

            platform = next(mac_platforms())
        return "im314", "imre1", platform

    def write_metadata(self, wheelfile_base, *args, **kwargs):
        # bdist_wheel writes RECORD after this hook, so the overlay and native
        # payload use the upstream wheel's normal installation scheme and hashes.
        root = Path(wheelfile_base).parent
        if overlay:
            for source in overlay.rglob("*"):
                if not source.is_file():
                    continue
                relative = source.relative_to(overlay)
                if (
                    source.suffix not in {".py", ".pyi"}
                    and source.name != "py.typed"
                    and "licenses" not in relative.parts[:-1]
                ):
                    raise ValueError(f"unsupported wheel overlay file: {relative}")
                dest = root / relative
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, dest)
        command = self.get_finalized_command("build_ext")
        packages = self.get_finalized_command("build_py")
        expected = set()
        for ext in self.distribution.ext_modules or ():
            fullname = command.get_ext_fullname(ext.name)
            module_path = Path(*fullname.split("."))
            expected.add(module_path.with_suffix(".imre.a"))
            stub = root / module_path.with_suffix(".pyi")
            if not stub.is_file():
                package, _, module = fullname.rpartition(".")
                source = Path(packages.get_package_dir(package)) / f"{module}.pyi"
                if not source.is_file():
                    raise ValueError(
                        f"missing Imre declarations for {fullname}: {stub.relative_to(root)}"
                    )
                shutil.copyfile(source, stub)
        if (
            not expected
            or expected != built
            or expected != {p.relative_to(root) for p in root.rglob("*.imre.a")}
        ):
            raise ValueError(
                "the wheel must contain every declared extension built against Imre's Python.h"
            )
        for path in root.rglob("*"):
            if {".so", ".pyd", ".dylib", ".dll"}.intersection(path.suffixes):
                raise ValueError(
                    f"unexpected shared library in Imre wheel: {path.relative_to(root)}"
                )
        upstream_metadata(self, wheelfile_base, *args, **kwargs)

    build_ext.get_ext_filename = lambda self, name: (
        os.path.join(*name.split(".")) + ".imre.a"
    )
    build_ext.build_extensions = build_extensions
    bdist_wheel.get_tag = get_tag
    bdist_wheel.write_wheelfile = write_metadata
