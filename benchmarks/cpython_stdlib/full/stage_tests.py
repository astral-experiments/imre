"""Assemble the checked-in CPython test fixtures into an empty source root."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil

HERE = Path(__file__).resolve().parent


def stage(destination):
    if destination.exists() and any(destination.iterdir()):
        raise ValueError(f"destination must be empty: {destination}")
    shared = HERE.parent / "UPSTREAM.json"
    pin = json.loads(shared.read_bytes())["commit"]
    # OS originally pinned its three companion suites in separate manifests.
    manifests = [shared, *sorted(HERE.glob("*/UPSTREAM.json")),
                 *(HERE / "os" / f"{name}.json" for name in ("genericpath", "os", "posix"))]
    seen = {}
    for manifest in manifests:
        data = json.loads(manifest.read_bytes())
        if data["commit"] != pin:
            raise ValueError(f"{manifest}: CPython commit mismatch")
        for filename, expected in data["files"].items():
            path = PurePosixPath(filename)
            if path.parts[:2] != ("Lib", "test") and path.parts[:1] != ("Tools",):
                continue
            if ".." in path.parts:
                raise ValueError(f"{manifest}: invalid fixture path {filename}")
            source = manifest.parent / "upstream" / path
            if not source.is_file():
                source = shared.parent / "upstream" / path
            content = source.read_bytes()
            blob = hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest()
            if blob != expected or seen.get(path, blob) != blob:
                raise ValueError(f"{manifest}: wrong source blob for {filename}")
            if path not in seen:
                target = destination / path
                target.parent.mkdir(exist_ok=True, parents=True)
                shutil.copy(source, target)
                seen[path] = blob
    return len(seen)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(f"Staged {stage(args.destination)} verified CPython test files")
