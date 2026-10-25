"""List annotationlib imports and qualified reads in the pinned CPython Lib tree."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent


def inventory(root, tree_json=None):
    commit = json.loads((HERE / "ANNOTATIONLIB.json").read_text())["commit"]
    if tree_json is None:
        tree = subprocess.check_output(["git", "-C", root, "ls-tree", "-r", commit, "Lib"], text=True)
        blobs = {name: meta.split()[2] for meta, name in (line.split("\t", 1) for line in tree.splitlines())}
    else:
        tree = json.loads(tree_json.read_text())
        if tree["truncated"] or tree["sha"] != commit:
            raise ValueError("expected the complete recursive Git tree for the pinned commit")
        blobs = {e["path"]: e["sha"] for e in tree["tree"] if e["type"] == "blob"}
    clients = []
    for name, blob in sorted(blobs.items()):
        if not name.startswith("Lib/") or not name.endswith(".py"):
            continue
        raw = (root / name).read_bytes()
        if b"annotationlib" not in raw:
            continue
        digest = hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest()
        if digest != blob:
            raise ValueError(f"{name}: source differs from {commit}")
        nodes = list(ast.walk(ast.parse(raw, filename=name)))
        imports, aliases = [], set()
        for node in nodes:
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.name == "annotationlib":
                        aliases.add(alias.asname or alias.name)
                        imports.append([node.lineno, "import " + (alias.asname or alias.name)])
            if isinstance(node, ast.ImportFrom) and node.module == "annotationlib":
                for alias in node.names:
                    imports.append([node.lineno, alias.name])
        if not imports:
            continue
        # typing installs the module in this lazy proxy on its first attribute read.
        if name == "Lib/typing.py":
            assert any(isinstance(n, ast.Assign) and isinstance(n.value, ast.Name)
                       and n.value.id in aliases and any(isinstance(t, ast.Name)
                       and t.id == "_lazy_annotationlib" for t in n.targets) for n in nodes)
            aliases.add("_lazy_annotationlib")
        qualified = {}
        for node in nodes:
            if (isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name)
                    and node.value.id in aliases):
                qualified.setdefault(node.attr, set()).add(node.lineno)
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id == "getattr" and node.args and isinstance(node.args[0], ast.Name)
                    and node.args[0].id in aliases):
                qualified.setdefault("<getattr>", set()).add(node.lineno)
        clients.append({"source": name, "blob": digest, "imports": sorted(imports),
                        "qualified": {k: sorted(v) for k, v in sorted(qualified.items())}})
    return {"commit": commit, "clients": clients}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cpython", type=Path)
    parser.add_argument("--tree-json", type=Path, help="GitHub recursive Git tree for a release archive")
    args = parser.parse_args()
    report = inventory(args.cpython, args.tree_json)
    # One source per line keeps the complete line-number inventory reviewable.
    print('{"commit": ' + json.dumps(report["commit"]) + ', "clients": [')
    print(",\n".join("  " + json.dumps(c) for c in report["clients"]))
    print("]}")
