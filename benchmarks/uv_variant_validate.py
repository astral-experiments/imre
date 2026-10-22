"""Check uv's original Variant and stable sort on its pinned download catalog."""

import argparse
import ast
import asyncio
import runpy
import subprocess
import tempfile
import textwrap
from collections import Counter
from hashlib import sha256
from pathlib import Path


CATALOG = Path(__file__).resolve().parents[1] / "examples/python_catalog"
IMPORTS = '''
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from sys import argv
'''

DRIVER = '''
@dataclass
class Row:
    index: int
    variant: Variant | None

def rank(row: Row) -> int:
    return variant_sort_key(row.variant)

def run(path: str) -> None:
    rows: list[Row] = []
    for index, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines()):
        rows.append(Row(index, Variant.from_build_options(line.split())))
    original = rows
    rows.sort(key=rank)
    for row in original:
        variant = row.variant
        name = variant.value if variant is not None else "none"
        print(f"{row.index} {rank(row)} {name}")

run(argv[1])
'''


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", default="clang")
    args = parser.parse_args()

    adapter = runpy.run_path(str(CATALOG / "upstream_adapter.py"))
    upstream = adapter["load_upstream"]()
    catalog = adapter["read_source"](CATALOG / "sample/python-build-standalone.ndjson.gz")
    client = adapter["OfflineClient"](catalog, upstream.CPythonFinder.NDJSON_URL)
    downloads = asyncio.run(upstream.CPythonFinder(client).find())
    source = Path(upstream.__file__).read_text()
    definitions = {
        node.name: textwrap.dedent(ast.get_source_segment(source, node, padded=True))
        for node in ast.walk(ast.parse(source))
        if isinstance(node, (ast.ClassDef, ast.FunctionDef))
        and node.name in {"Variant", "variant_sort_key"}
    }
    # uv defines this key inside render(). Run the same body on the upstream members.
    key_scope = {"Variant": upstream.Variant}
    exec(definitions["variant_sort_key"], key_scope)
    sort_key = key_scope["variant_sort_key"]
    ordered = sorted(enumerate(downloads), key=lambda item: sort_key(item[1].variant))
    counts = Counter(sort_key(d.variant) for d in downloads)
    if counts.keys() != {0, 1, 2, 3}:
        raise AssertionError("the pinned catalog must exercise every variant rank")
    expected = "".join(
        f"{index} {sort_key(d.variant)} {d.variant.value if d.variant is not None else 'none'}\n"
        for index, d in ordered
    ).encode()
    options = [download.build_options for download in downloads]
    if any(part.split() != [part] for parts in options for part in parts):
        raise ValueError("the line fixture requires single-token build options")

    with tempfile.TemporaryDirectory(prefix="relar-uv-variant-") as tmp:
        root = Path(tmp)
        program, inputs = root / "variant.py", root / "options.txt"
        program.write_text(
            "\n\n".join([IMPORTS, definitions["Variant"], definitions["variant_sort_key"], DRIVER])
        )
        inputs.write_text("".join(" ".join(parts) + "\n" for parts in options))
        for opt in ("0", "3"):
            executable = root / f"variant-O{opt}"
            subprocess.run(
                [
                    args.compiler.resolve(), "build", program, "--opt-level", opt,
                    "--clang", args.clang, "--runtime", args.runtime.resolve(), "-o", executable,
                ],
                check=True,
            )
            actual = subprocess.run([executable, inputs], check=True, capture_output=True)
            if actual.stdout != expected or actual.stderr:
                raise AssertionError(f"O{opt} disagreed with uv's CPython sort")
    print(
        f"{len(downloads)} downloads, ranks {dict(sorted(counts.items()))}, "
        f"O0/O3 SHA-256 {sha256(expected).hexdigest()}"
    )


if __name__ == "__main__":
    main()
