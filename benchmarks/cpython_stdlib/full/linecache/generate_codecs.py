"""Generate native PEP 263 name and single-byte decoding tables from the pinned interpreter."""

import argparse
import codecs
import encodings
import encodings.aliases
import importlib
from pathlib import Path
import pkgutil
import struct
import sys

if sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
    raise SystemExit("run with pinned Linux CPython 3.14.5")
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
out = Path(__file__).resolve().parents[4] / "crates/imre-runtime/src/objects/linecache"
modules = {item.name for item in pkgutil.iter_modules(encodings.__path__)}
names = sorted(modules | encodings.aliases.aliases.keys())
tables, aliases = [], {}
for name in names:
    try:
        codec = codecs.lookup(name)
    except LookupError:
        continue
    source = encodings.aliases.aliases.get(name, name)
    try:
        module = importlib.import_module("encodings." + source)
    except ImportError:
        module = None
    if not codec._is_text_encoding:
        kind = "nontext"
    elif codec.name in {"utf-8", "utf-8-sig", "ascii", "iso8859-1"}:
        kind = codec.name
    elif isinstance(getattr(module, "decoding_table", None), str):
        table = module.decoding_table
        assert len(table) == 256 and max(map(ord, table)) < 65536
        if table not in tables:
            tables.append(table)
        kind = str(tables.index(table))
    else:
        kind = "unsupported"
    for alias in [name, codec.name]:
        alias = encodings.normalize_encoding(alias).lower()
        assert alias not in aliases or aliases[alias] == kind, (alias, kind)
        aliases[alias] = kind
data = {
    "codec_names.txt": "".join(
        f"{alias} {kind}\n" for alias, kind in sorted(aliases.items())
    ).encode(),
    "charmaps.bin": b"".join(struct.pack("<256H", *map(ord, t)) for t in tables),
}
for filename, content in data.items():
    path = out / filename
    if args.check:
        if path.read_bytes() != content:
            raise SystemExit(
                f"pinned {filename} differs; regenerate with Linux CPython 3.14.5"
            )
    else:
        path.write_bytes(content)
print(f"{len(aliases)} registered names, {len(tables)} single-byte tables")
