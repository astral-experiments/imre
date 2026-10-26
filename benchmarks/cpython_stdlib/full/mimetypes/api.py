"""Compare every pinned built-in mapping and the POSIX host database with ordinary CPython."""
import argparse
import ast
from hashlib import sha256
import json
import mimetypes
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
from bridge import worker
from suite_inventory import verify_files
from suite_native import Toolchain

CASES = ("", ".html", ".tar.gz", "a/..name.txt", "a.tar.Z", "a.tar.z", "a.tar.GZ",
         "a.TGZ", "a.SVGZ", "a.tar.gz.xz", "a..bz2", "a#b.txt", "photo.svg?other=.gz",
         "c:/directory/file.jpg#last", "//host.html", "http://host.html",
         "https://host/x.JPG?name=file.gz#frag", "http://host/a;dir/b.txt;v=x.gz",
         "scheme:a.txt;v=x.gz", "data:bad", "data:,text", "data:;base64,xxx",
         "DATA:image/svg+xml;charset=UTF-8,svg", "data:charset=UTF-8,text",
         "ftp://host/dir/file.TXZ;type=I", "é/β.TGZ", "packet.cbor", "app.deb")


def main():
    args = argparse.ArgumentParser(description=__doc__)
    for a in ("imre", "runtime", "clang", "output"):
        args.add_argument("--" + a, required=True, type=Path)
    args = args.parse_args()
    assert sys.version_info[:3] == (3, 14, 5) and sys.implementation.name == "cpython"
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    assert sha256(Path(mimetypes.__file__).read_bytes()).digest() == sha256((HERE / "upstream/Lib/mimetypes.py").read_bytes()).digest()
    tables = {"_suffixes": "_suffix_map_default", "_encodings": "_encodings_map_default",
              "_standard": "_types_map_default", "_common": "_common_types_default"}
    native = ast.parse((HERE.parents[3] / "crates/imre-compiler/stdlib/mimetypes.py").read_bytes())
    for node in native.body:
        if isinstance(node, ast.FunctionDef) and node.name in tables:
            assert list(ast.literal_eval(node.body[0].value).items()) == list(getattr(mimetypes, tables.pop(node.name)).items())
    assert not tables
    report = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        custom = root / "custom.types"
        custom.write_text("# custom table\napplication/first imre-custom\napplication/winner imre-custom imre-common # ignored\n", encoding="utf-8")
        files = [custom, HERE / "upstream/Lib/test/mime.types"]
        config = HERE / "configuration.py"
        expected = subprocess.run([sys.executable, "-IB", config, *files], capture_output=True, check=True)
        for opt in (0, 3):
            for reachable in (False, True):
                tool = Toolchain(args.imre, args.runtime, args.clang, opt, reachable)
                program = tool.build(config.read_bytes(), root)
                actual = subprocess.run([program, *files], capture_output=True, check=True)
                assert (actual.stdout, actual.stderr) == (expected.stdout, expected.stderr), (actual, expected)
                program = tool.build((HERE / "worker.py").read_bytes(), root)
                for mode in ("builtins", "system"):
                    with patch.object(mimetypes, "knownfiles", [] if mode == "builtins" else mimetypes.knownfiles):
                        mimetypes.init()
                        extensions = sorted(set(mimetypes.types_map) | set(mimetypes.common_types) | set(mimetypes.suffix_map) | set(mimetypes.encodings_map))
                        host = {p: sha256(Path(p).read_bytes()).hexdigest() for p in mimetypes.knownfiles if Path(p).is_file()}
                        count = 0
                        digest = sha256()
                        with worker(program, mode) as native_call:
                            for name in ["file" + ext for ext in extensions] + list(CASES):
                                for op, reference in (("url", mimetypes.guess_type), ("file", mimetypes.guess_file_type)):
                                    for strict in (True, False):
                                        value = native_call(op, name, strict)
                                        wanted = reference(name, strict=strict)
                                        if value != wanted:
                                            raise ValueError((opt, reachable, mode, op, name, strict, value, wanted))
                                        digest.update((repr((op, name, strict, value)) + "\n").encode())
                                        count += 1
                        report.append(dict(opt_level=opt, reachable_only=reachable, configuration=mode,
                                           mapping_extensions=len(extensions), lookups=count, output_sha256=digest.hexdigest(),
                                           configuration_stdout_sha256=sha256(expected.stdout).hexdigest(),
                                           knownfile_sha256=host))
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
