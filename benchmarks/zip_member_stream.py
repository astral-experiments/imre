"""Measure stored and DEFLATE ZIP reads under a POSIX address-space limit (Linux)."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import sys
import tempfile
import time
import zipfile

DRIVER = '''import hashlib
import sys
from zipfile import ZipFile

digest = hashlib.sha256()
count = 0
with ZipFile(sys.argv[1]) as archive:
    with archive.open("payload") as member:
        while True:
            chunk = member.read(65536)
            if not chunk:
                break
            digest.update(chunk)
            count += len(chunk)
print(str(count) + " " + digest.hexdigest())
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--size-mib", type=int, default=256)
    p.add_argument("--address-limit-mib", type=int, default=64)
    p.add_argument("--runs", type=int, default=5)
    args = p.parse_args()
    if platform.system() != "Linux" or min(args.size_mib, args.address_limit_mib, args.runs) < 1:
        p.error("requires Linux, GNU /usr/bin/time, and positive sizes")
    limit = args.address_limit_mib * 1024**2
    def restrict():
        resource.setrlimit(resource.RLIMIT_AS, (limit, limit))
    report = {"python": sys.version, "platform": platform.platform(), "size_mib": args.size_mib,
              "address_limit_mib": args.address_limit_mib, "chunk_bytes": 65536, "methods": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        src, native = work / "stream.py", work / "native"
        src.write_text(DRIVER)
        subprocess.run([args.imre.resolve(), "build", src, "--no-cache", "--opt-level", "3", "--runtime",
                        args.runtime.resolve(), "--clang", args.clang.resolve(), "-o", native], check=True, timeout=120)
        block = bytes(range(256)) * 4096
        digest = hashlib.sha256()
        for _ in range(args.size_mib):
            digest.update(block)
        expected = (str(args.size_mib * len(block)) + " " + digest.hexdigest() + "\n").encode()
        for method in ("stored", "deflate"):
            fixture = work / (method + ".zip")
            with zipfile.ZipFile(fixture, "w", compression={"stored": zipfile.ZIP_STORED, "deflate": zipfile.ZIP_DEFLATED}[method]) as archive:
                with archive.open("payload", "w") as stream:
                    for _ in range(args.size_mib):
                        stream.write(block)
            samples = {"cpython": [], "native": []}
            runs = {"cpython": [sys.executable, "-B", src], "native": [native]}
            for rep in range(args.runs + 1):
                for name in (("cpython", "native") if rep % 2 == 0 else ("native", "cpython")):
                    rss = work / "rss"
                    start = time.perf_counter()
                    child = subprocess.run(["/usr/bin/time", "-o", rss, "-f", "%M", "--", *runs[name], fixture],
                                           preexec_fn=restrict, capture_output=True, timeout=120)
                    elapsed = time.perf_counter() - start
                    if child.returncode or child.stdout != expected or child.stderr:
                        raise AssertionError((name, child, expected))
                    if rep:
                        samples[name].append({"seconds": elapsed, "peak_rss_kib": int(rss.read_text())})
            report["methods"][method] = {
                "archive_bytes": fixture.stat().st_size, "sha256": digest.hexdigest(), "samples": samples,
                "medians": {k: {"seconds": statistics.median(v["seconds"] for v in seq),
                                "peak_rss_kib": statistics.median(v["peak_rss_kib"] for v in seq)}
                            for k, seq in samples.items()},
            }
            print(method, report["methods"][method]["medians"])
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
