"""Exercise Linux linker options against the shared native executable and object caches."""

import argparse
import os
from pathlib import Path
import re
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
        r"^\s*\d+:\s*\w+\s+\d+\s+FUNC\s+GLOBAL\s+\S+\s+\d+\s+(relar_\w+)$",
        readelf(path, "--symbols"), re.MULTILINE,
    ))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    compiler = parser.parse_args().compiler.resolve()
    if not __debug__:
        parser.error("run with assertions enabled")
    with tempfile.TemporaryDirectory(prefix="relar-link-cache-") as td:
        root = Path(td)
        environment = dict(os.environ, RELAR_CACHE_DIR=str(root / "cache"), RELAR_CACHE_TRACE="1")
        for key in ("RELARFLAGS", "RELAR_ENCODED_FLAGS"):
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
            assert [s.removeprefix("relar cache ") for s in lines if s.startswith("relar cache ")] == events, lines
            if objects is not None:
                assert {s for s in lines if s.startswith("relar object ")} == {
                    f"relar object {objects} {module}" for module in ("main", "leaf")
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
        invoke(events=["hit"], RELARFLAGS="--no-gc-sections")
        for level in ("partial", "off"):
            option = f"relro-level={level}"
            invoke("-C", option, events=miss, objects="hit")
            assert relro(binary) == level
            invoke(events=["hit"], RELARFLAGS=f"-C {option}")
            invoke(events=["hit"], RELARFLAGS="--invalid", RELAR_ENCODED_FLAGS=f"-C\x1f{option}")
        invoke("--gc-sections", "-Crelro-level=full", events=["hit"],
               RELARFLAGS="--no-gc-sections -C relro-level=off")
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
        invoke(events=["bypass"], RELAR_ENCODED_FLAGS=option)
        assert subprocess.check_output([binary], text=True) == "second\n17\n"
        invoke(events=["hit"], mode="run")
    print("Passed native link option, cache identity, and external input checks")


if __name__ == "__main__":
    main()
