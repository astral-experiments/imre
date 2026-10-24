"""Verify the checked-in CPython release files against their upstream Git blobs."""
import hashlib
import json
from pathlib import Path


def verify_upstream():
    here = Path(__file__).resolve().parent
    manifest = json.loads((here / "UPSTREAM.json").read_text())
    for name, oid in manifest["files"].items():
        data = (here / "upstream" / name).read_bytes()
        header = f"blob {len(data)}\0".encode()
        if hashlib.sha1(header + data).hexdigest() != oid:
            raise ValueError(f"modified upstream file: {name}")
    return manifest["commit"]
