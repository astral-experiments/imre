"""Exercise Linux linker options against the shared native executable and object caches."""

import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def readelf(path, option):
    return subprocess.check_output(["readelf", "-W", option, path], text=True)


def relro(path):
    protected = "GNU_RELRO" in readelf(path, "--program-headers")
    if not protected:
        return "off"
    return "full" if re.search(r"\b(?:BIND_NOW|NOW)\b", readelf(path, "--dynamic")) else "partial"


def runtime_functions(path):
    return set(re.findall(
        r"^\s*\d+:\s*\w+\s+\d+\s+FUNC\s+GLOBAL\s+\S+\s+\d+\s+(imre_\w+)$",
        readelf(path, "--symbols"), re.MULTILINE,
    ))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    compiler = parser.parse_args().compiler.resolve()
    if not __debug__:
        parser.error("run with assertions enabled")
    with tempfile.TemporaryDirectory(prefix="imre-link-cache-") as td:
        root = Path(td)
        environment = dict(os.environ, IMRE_CACHE_DIR=str(root / "cache"), IMRE_CACHE_TRACE="1")
        for key in ("IMREFLAGS", "IMRE_ENCODED_FLAGS"):
            environment.pop(key, None)
        (root / "main.py").write_text("from leaf import amount\nprint(amount())\n")
        (root / "leaf.py").write_text("def amount() -> int:\n    return 17\n")
        binary = root / "program"

        def invoke(*flags, events, objects=None, mode="build", **env):
            command = [compiler, mode, "main.py", "--quiet", *flags]
            if mode == "build":
                command += ["-o", binary]
            result = subprocess.run(command, cwd=root, env=environment | env, text=True, capture_output=True)
            assert result.returncode == 0, (command, result.stderr)
            lines = result.stderr.splitlines()
            assert [s.removeprefix("imre cache ") for s in lines if s.startswith("imre cache ")] == events, lines
            if objects is not None:
                assert {s for s in lines if s.startswith("imre object ")} == {
                    f"imre object {objects} {module}" for module in ("main", "leaf")
                }, lines
            assert result.stdout == ("17\n" if mode == "run" else ""), result.stdout
            return lines

        miss = ["miss", "store"]
        invoke(events=miss, objects="miss")
        assert relro(binary) == "full"
        collected = runtime_functions(binary)
        assert collected
        invoke("--gc-sections", "-Crelro-level=full", events=["hit"], mode="run")

        # Link-only changes reuse both compiled modules, but produce a different ELF.
        invoke("--no-gc-sections", events=miss, objects="hit")
        assert collected < runtime_functions(binary)
        invoke(events=["hit"], IMREFLAGS="--no-gc-sections")
        for level in ("partial", "off"):
            option = f"relro-level={level}"
            invoke("-C", option, events=miss, objects="hit")
            assert relro(binary) == level
            invoke(events=["hit"], IMREFLAGS=f"-C {option}")
            invoke(events=["hit"], IMREFLAGS="--invalid", IMRE_ENCODED_FLAGS=f"-C\x1f{option}")
        invoke("--gc-sections", "-Crelro-level=full", events=["hit"],
               IMREFLAGS="--no-gc-sections -C relro-level=off")
        assert relro(binary) == "full" and runtime_functions(binary) == collected

        # A fresh staging directory must produce an ELF and linker depfile to store the result.
        verbose = invoke("--verbose-link", events=["relink", "store"], objects="hit")
        assert any("clang-18" in line and "-Wl,--gc-sections" in line and "-Wl,-z,relro,-z,now" in line
                   for line in verbose), verbose
        assert subprocess.check_output([binary], text=True) == "17\n"
        invoke(events=["hit"])

        # A user-supplied .o can change beneath an otherwise identical command line.
        external = root / "extra object.o"
        option = f"-Clink-arg={external}"
        for marker in ("first", "second"):
            program = (
                '#include <unistd.h>\n'
                f'__attribute__((constructor)) static void start(void) {{ '
                f'write(1, "{marker}\\n", {len(marker) + 1}); }}\n'
            )
            subprocess.run(["clang-18", "-x", "c", "-c", "-", "-o", external], input=program,
                           text=True, check=True)
            invoke(option, events=["bypass"])
            assert subprocess.check_output([binary], text=True) == f"{marker}\n17\n"
        invoke(events=["bypass"], IMRE_ENCODED_FLAGS=option)
        assert subprocess.check_output([binary], text=True) == "second\n17\n"
        invoke(events=["hit"], mode="run")

        # Moving from the system linker to the packaged one changes only the link action.
        source_lld = next((p for p in (compiler.with_name("imre-ld.lld"), Path("/usr/bin/ld.lld-18"))
                           if p.is_file()), None)
        if source_lld:
            install = root / "install"
            install.mkdir()
            (install / "libimre_runtime.a").symlink_to(compiler.with_name("libimre_runtime.a"))
            compiler = Path(shutil.copy2(compiler, install / "imre"))
            invoke(events=miss, objects="miss")
            invoke(events=["hit"], mode="run")
            companion = Path(shutil.copy2(source_lld, install / "imre-ld.lld"))
            invoke(events=miss, objects="hit")
            verbose = invoke("--verbose-link", events=["relink", "store"], objects="hit")
            assert any(f"--ld-path={companion}" in line for line in verbose), verbose
            stat = companion.stat()
            os.utime(companion, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))
            invoke(events=miss, objects="hit")
            invoke(events=["hit"], mode="run")
            collision = subprocess.run([compiler, "build", "main.py", "-o", companion], cwd=root,
                                       env=environment, capture_output=True, text=True)
            assert collision.returncode and "distinct paths" in collision.stderr, collision.stderr

            # A packaged driver takes precedence over PATH, also on the uncached IR path.
            tools = install / "imre-toolchain"
            for name in ("bin", "lib", "link"):
                (tools / name).mkdir(parents=True)
            private_clang = Path(shutil.copy2("/usr/bin/clang-18", tools / "bin/clang"))
            (tools / "bin/ld.lld").symlink_to(companion)
            zlib = subprocess.check_output(["clang-18", "-print-file-name=libz.so.1"], text=True).strip()
            library = Path(shutil.copy2(zlib, tools / "lib/libz.so.1"))
            without_clang = {"PATH": str(root / "empty-path")}
            verbose = invoke("--verbose-link", events=["relink", "store"], objects="miss", **without_clang)
            assert any(str(private_clang) in line and str(tools / "bin/ld.lld") in line for line in verbose)
            invoke(mode="run", events=["hit"], **without_clang)
            stamp = library.stat()
            os.utime(library, ns=(stamp.st_atime_ns, stamp.st_mtime_ns + 1_000_000_000))
            invoke(events=miss, objects="miss", **without_clang)
            invoke("--emit-llvm", root / "output.ll", mode="run", events=["bypass"], **without_clang)
            invoke("--target", f"{os.uname().machine}-linux-gnu", mode="run", events=["bypass"], **without_clang)
            override = invoke("-Clink-arg=--ld-path=/usr/bin/ld", "--verbose-link",
                              mode="run", events=["bypass"], **without_clang)
            assert all(str(tools / "bin/ld.lld") not in line for line in override), override
            explicit = invoke("--clang", "/usr/bin/clang-18", "--verbose-link", events=["bypass"])
            assert all(str(tools) not in line for line in explicit), explicit
    print("Passed native link option, cache identity, and external input checks")


if __name__ == "__main__":
    main()
