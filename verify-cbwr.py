"""Verify a cbwr archive using a separately obtained receipt signer pin."""
import argparse
import gzip
import hashlib
import re
import sys
import tarfile

from cryptography.exceptions import InvalidSignature, UnsupportedAlgorithm
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


PREFIX = "cbwr/"
MANIFEST = "SHA256SUMS"
SIGNATURE = "SHA256SUMS.sig"
PUBLIC_KEY = "signer-ed25519.pem"
DIGEST = re.compile(r"[0-9a-f]{64}")
PART = re.compile(r"[a-zA-Z0-9_][a-zA-Z0-9_. +@-]*")
LIMITS = {MANIFEST: 16 * 1024 * 1024, SIGNATURE: 64, PUBLIC_KEY: 4096}
CHUNK = 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_path(name):
    for part in name.split("/"):
        require(PART.fullmatch(part) and not part.endswith((" ", ".")),
                f"unsafe receipt path {name!r}")


def read_archive(path):
    hashes, metadata, seen = {}, {}, set()
    with gzip.open(path, "rb") as compressed:
        with tarfile.open(fileobj=compressed, mode="r:") as archive:
            for member in archive:
                require(member.type in (tarfile.REGTYPE, tarfile.AREGTYPE)
                        and member.sparse is None and member.size >= 0,
                        f"non-regular archive member {member.name!r}")
                require(member.name.startswith(PREFIX), f"unexpected prefix in {member.name!r}")
                name = member.name[len(PREFIX):]
                validate_path(name)
                require(name not in seen, f"duplicate archive member {name!r}")
                seen.add(name)
                with archive.extractfile(member) as stream:
                    if name in LIMITS:
                        require(0 < member.size <= LIMITS[name], f"invalid {name} size")
                        metadata[name] = stream.read()
                        if name == PUBLIC_KEY:
                            hashes[name] = hashlib.sha256(metadata[name]).hexdigest()
                    else:
                        digest = hashlib.sha256()
                        for chunk in iter(lambda: stream.read(CHUNK), b""):
                            digest.update(chunk)
                        hashes[name] = digest.hexdigest()

            # tarfile accepts a missing end marker and stops before appended members.
            compressed.seek(archive.offset)
            tail_size = 0
            for tail in iter(lambda: compressed.read(CHUNK), b""):
                require(tail.count(0) == len(tail), "data after final archive member")
                tail_size += len(tail)
            require(tail_size >= 1024 and tail_size % 512 == 0, "invalid tar end marker")

    for name in seen:
        parent, sep, _ = name.rpartition("/")
        while sep:
            require(parent not in seen, f"file also used as directory {parent!r}")
            parent, sep, _ = parent.rpartition("/")
    require(metadata.keys() == LIMITS.keys(), "missing signature, manifest, or public key")
    return hashes, metadata


def verify(path, pin):
    require(DIGEST.fullmatch(pin), "--pin must be the 64 lowercase hex digits of SHA-256(SPKI DER)")
    actual, metadata = read_archive(path)
    pem = metadata[PUBLIC_KEY]
    require(re.fullmatch(rb"-----BEGIN PUBLIC KEY-----\r?\n(?:[A-Za-z0-9+/=]+\r?\n)+"
                         rb"-----END PUBLIC KEY-----(?:\r?\n)?", pem), "invalid SPKI PEM")
    key = serialization.load_pem_public_key(pem)
    require(isinstance(key, Ed25519PublicKey), "signer is not an Ed25519 key")
    der = key.public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo)
    require(hashlib.sha256(der).hexdigest() == pin, "receipt signer pin mismatch")

    manifest, signature = metadata[MANIFEST], metadata[SIGNATURE]
    require(len(signature) == 64, "invalid Ed25519 signature length")
    try:
        key.verify(signature, manifest)
    except InvalidSignature:
        raise ValueError("invalid Ed25519 signature") from None

    listed = {}
    lines = manifest.decode("ascii").split("\n")
    require(lines.pop() == "", "manifest must end with LF")
    for line in lines:
        digest, separator, name = line.partition("  ")
        require(DIGEST.fullmatch(digest) and separator, "malformed SHA256SUMS entry")
        validate_path(name)
        require(name not in (MANIFEST, SIGNATURE), f"reserved manifest entry {name!r}")
        require(name not in listed, f"duplicate manifest entry {name!r}")
        listed[name] = digest
    require(actual.keys() == listed.keys(), "archive member set differs from signed manifest")
    for name, expected in listed.items():
        require(actual[name] == expected, f"SHA-256 mismatch for {name!r}")
    return len(listed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", help="cbwr .tar.gz")
    parser.add_argument("--pin", required=True, help="externally supplied receipt signer SHA-256(SPKI DER)")
    args = parser.parse_args()
    try:
        count = verify(args.archive, args.pin)
    except (OSError, EOFError, ValueError, UnsupportedAlgorithm, tarfile.TarError) as exc:
        print(f"Verification failed: {exc}", file=sys.stderr)
        return 1
    print(f"Verified {count} payloads with receipt signer SHA-256(SPKI DER) {args.pin}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
