"""Assemble Relar's relocatable native LLVM 18 tools on the Ubuntu release runner."""

import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def bundle(dest):
    bin_dir, lib, link, licenses = (dest / name for name in ("bin", "lib", "link", "licenses"))
    for directory in (bin_dir, lib, link, licenses):
        directory.mkdir(parents=True)
    arch = platform.machine()
    multiarch = output("gcc", "-print-multiarch")
    distro_lib = Path("/usr/lib") / multiarch
    llvm = Path("/usr/lib/llvm-18")
    packages = {"clang-18", "lld-18", "libc6-dev", "libclang-rt-18-dev"}
    tools = [llvm / "bin" / name for name in ("clang", "ld.lld")]

    # ldd includes dependencies of dependencies. Leave glibc and the GCC unwinder
    # to the host, which already supplies both to run the Relar executable.
    host = {"libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1", "libgcc_s.so.1"}
    libraries = {}
    for tool in tools:
        shutil.copy2(tool, bin_dir / tool.name)
        for line in output("ldd", tool).splitlines():
            if "=>" in line:
                name, _, path = line.strip().partition(" => ")
                if not path.startswith("/"):
                    raise ValueError(f"unresolved dependency: {line}")
                if name not in host:
                    libraries[name] = Path(path.split(" (", 1)[0]).resolve(strict=True)
    for name, path in sorted(libraries.items()):
        target = lib / name
        shutil.copy2(path, target)
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", target], check=True)
        packages.add(output("dpkg-query", "-S", path).split(": ", 1)[0].split(":")[0])
    for tool in tools:
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN/../lib", bin_dir / tool.name], check=True)

    # These inputs normally come from libc6-dev and libgcc-*-dev. Use the
    # installed host's versioned glibc DSOs at link time and at program startup.
    gcc_lib = Path(output("gcc", "-print-libgcc-file-name")).parent
    packages.add(output("dpkg-query", "-S", gcc_lib / "libgcc.a").split(": ", 1)[0].split(":")[0])
    for filename in ("Scrt1.o", "crt1.o", "crti.o", "crtn.o", "libc_nonshared.a", "libdl.a",
                     "libpthread.a", "librt.a", "libutil.a"):
        shutil.copy2(distro_lib / filename, link)
    for filename in ("crtbeginS.o", "crtendS.o", "crtbegin.o", "crtend.o", "libgcc.a"):
        shutil.copy2(gcc_lib / filename, link)
    # Keep GROUP and AS_NEEDED ordering, but resolve the host's versioned DSOs
    # and our private libc_nonshared.a through the linker's search paths.
    for path, soname in ((distro_lib / "libc.so", "libc.so.6"), (distro_lib / "libm.so", "libm.so.6"),
                         (gcc_lib / "libgcc_s.so", "libgcc_s.so.1")):
        if path.is_symlink():
            script = f"INPUT ( {soname} )\n"
        else:
            script = re.sub(r"/(?:[\w.+-]+/)+([\w.+-]+)", r"\1", path.read_text())
        (link / path.name).write_text(script)
    (link / "libresolv.so").write_text("INPUT ( libresolv.so.2 )\n")

    # Relar hands Clang LLVM IR; its address-sanitizer mode also needs compiler-rt.
    runtimes = Path("clang/18/lib/linux")
    (lib / runtimes).mkdir(parents=True)
    for filename in (f"libclang_rt.asan-{arch}.a", f"libclang_rt.asan-{arch}.a.syms",
                     f"libclang_rt.asan_static-{arch}.a"):
        shutil.copy2(llvm / "lib" / runtimes / filename, lib / runtimes)

    sources = []
    for package in sorted(packages):
        shutil.copyfile(Path("/usr/share/doc") / package / "copyright", licenses / package)
        source, version = output("dpkg-query", "-W", "-f=${source:Package} ${source:Version}", package).split()
        sources.append(f"{package}\thttps://launchpad.net/ubuntu/+source/{source}/{version}")
    (licenses / "sources.tsv").write_text("\n".join(sources) + "\n")
    shutil.copytree("/usr/share/common-licenses", licenses / "common-licenses")
    for notice in (Path(__file__).resolve().parents[1] / "licenses").glob("*.txt"):
        shutil.copy2(notice, licenses)


if __name__ == "__main__":
    bundle(Path(sys.argv[1]))
