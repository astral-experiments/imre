"""Assemble the checked-in CPython test fixtures into an empty source root."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil

HERE = Path(__file__).resolve().parent


def git_blob(path):
    data = path.read_bytes()
    return hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()


def verified_sources():
    shared = HERE.parent / "UPSTREAM.json"
    pin = json.loads(shared.read_bytes())["commit"]
    # Include companion suites such as urllib's independently pinned test_urllib.
    manifests = [shared, *sorted(HERE.glob("*/UPSTREAM.json")), *sorted(HERE.glob("*/UPSTREAM-*.json")),
                 *sorted(HERE.glob("*/FREE_THREADING.json")),
                 *(HERE / "os" / f"{name}.json" for name in ("genericpath", "os", "posix")),
                 *(HERE / "http" / f"{name}.json" for name in ("httplib", "httpservers", "http_cookies", "http_cookiejar"))]
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
            if git_blob(source) != expected or (path in seen and seen[path][1] != expected):
                raise ValueError(f"{manifest}: wrong source blob for {filename}")
            seen[path] = source, expected
    return seen


def stage(destination):
    if destination.exists() and any(destination.iterdir()):
        raise ValueError(f"destination must be empty: {destination}")
    sources = verified_sources()
    for path, (source, _) in sources.items():
        target = destination / path
        target.parent.mkdir(exist_ok=True, parents=True)
        shutil.copy(source, target)
    return len(sources)


def verify_prefix(stdlib):
    """Reject a host test package, an extra fixture, or a modified staged blob."""
    stdlib = stdlib.resolve()
    sources = verified_sources()
    expected = set()
    for path, (_, blob) in sources.items():
        target = (stdlib.joinpath(*path.parts[1:]) if path.parts[0] == "Lib"
                  else stdlib.parent / path)
        if not target.is_file() or git_blob(target) != blob:
            raise ValueError(f"missing or modified installed fixture: {path}")
        expected.add(target)
    for root in (stdlib / "test", stdlib.parent / "Tools"):
        for target in root.rglob("*"):
            if target.is_file() and "__pycache__" not in target.relative_to(root).parts and target not in expected:
                raise ValueError(f"unexpected installed fixture: {target}")
    return len(sources)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(f"Staged {stage(args.destination)} verified CPython test files")
