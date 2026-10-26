"""Run the four unchanged wheel consumers with CPython or a native MIME provider."""
import argparse
from collections import Counter
from hashlib import sha256
from io import BytesIO
import json
import mimetypes
from pathlib import Path
import runpy
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parents[1]))
from bridge import worker
from suite_native import Toolchain

NAMES = ("page.HTML", "photo.jpg", "report.txt.gz", "archive.TGZ", "archive.tgz", "logo.svgz",
         "payload.imreunknown", "styles.css?version=2", "description.md", "café.txt", "plain",
         "packet.cbor", "app.deb")
STATIC = ("page.HTML", "photo.jpg", "report.txt.gz", "archive.TGZ", "logo.svgz",
          "payload.imreunknown", "packet.cbor", "app.deb")


def exercise(site, files, provider):
    from werkzeug.datastructures import FileMultiDict, FileStorage
    from werkzeug.middleware.shared_data import SharedDataMiddleware
    from werkzeug.test import Client, stream_encode_multipart
    from werkzeug.utils import send_file
    from werkzeug.wrappers import Response

    calls = []
    def guess(url, strict=True):
        caller = sys._getframe(1).f_code
        source = Path(caller.co_filename).relative_to(site).as_posix()
        result = provider(url, strict)
        calls.append(dict(source=source, call=caller.co_name, filename=url, result=list(result)))
        return result

    outputs = {k: {} for k in ("upload", "multipart", "send_file", "static")}
    with patch.object(mimetypes, "guess_type", guess):
        for name in NAMES:
            uploads = FileMultiDict()
            uploads.add_file("f", BytesIO(b"body"), name)
            upload = uploads["f"]
            outputs["upload"][name] = [upload.content_type, upload.filename, upload.read().decode()]
            upload.close()
            file = FileStorage(BytesIO(b"body"), filename=name)
            stream, _, _ = stream_encode_multipart({"f": file}, use_tempfile=False, boundary="imre_test")
            headers = stream.read().partition(b"\r\n\r\n")[0].split(b"\r\n")
            [content_type] = [h.partition(b": ")[2] for h in headers if h.startswith(b"Content-Type: ")]
            outputs["multipart"][name] = content_type.decode()
            file.close()
            stream.close()
            response_types = {}
            for label in ("inline", "attachment"):
                response = send_file(BytesIO(b"body"), {}, download_name=name, as_attachment=label == "attachment",
                                     conditional=False, etag=False)
                response_types[label] = [response.content_type, response.headers.get("Content-Encoding")]
                response.close()
            outputs["send_file"][name] = response_types

        supplied = FileMultiDict()
        before = len(calls)
        supplied.add_file("f", BytesIO(b"x"), "page.HTML", "x/imre")
        assert supplied["f"].content_type == "x/imre" and len(calls) == before
        supplied["f"].close()
        app = SharedDataMiddleware(lambda e, s: Response("missing", status=404)(e, s), {"/files": str(files)}, cache=False)
        client = Client(app, Response)
        for name in STATIC:
            (files / name).write_bytes(b"body")
            response = client.get("/files/" + name)
            assert response.status_code == 200 and response.get_data() == b"body"
            outputs["static"][name] = [response.status_code, response.content_type, response.headers.get("Content-Encoding")]
            response.close()
    assert len(calls) == 60 and len({c["source"] for c in calls}) == 4
    return {"consumers": outputs, "guess_type_calls": calls}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "wheelhouse", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--reference", type=Path, help="compare the independent Flask milestone receipt on its recorded host")
    args = parser.parse_args()
    assert sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 5)
    reference = json.loads(args.reference.read_bytes()) if args.reference else None
    stage = runpy.run_path(str(HERE.parents[2] / "flask/gate.py"))["stage"]
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        site = work / "site"
        site.mkdir()
        wheels = stage(args.wheelhouse, site)
        sys.path.insert(0, str(site))
        files = work / "static"
        files.mkdir()
        binary = {}
        for level in (0, 3):
            for reachable in (False, True):
                dest = work / f"{level}-{int(reachable)}"
                dest.mkdir()
                binary[level, reachable] = Toolchain(args.imre, args.runtime, args.clang, level, reachable).build(
                    (HERE / "worker.py").read_bytes(), dest)
        report = {}
        for mode in ("system", "builtins"):
            with patch.object(mimetypes, "knownfiles", mimetypes.knownfiles if mode == "system" else []):
                mimetypes.init()
                expected = exercise(site, files, mimetypes.guess_type)
                db = {n: sha256(Path(n).read_bytes()).hexdigest() for n in mimetypes.knownfiles if Path(n).is_file()}
            sources = {c["source"]: sha256((site / c["source"]).read_bytes()).hexdigest() for c in expected["guess_type_calls"]}
            if reference:
                original = reference[mode]
                assert wheels == original["wheels"] and sources == original["sources"] and db == original["mime_database_file_hashes"]
                assert all(expected[k] == original[k] for k in expected)
            rows = []
            for (level, reachable), exe in binary.items():
                with worker(exe, mode) as native:
                    actual = exercise(site, files, lambda url, strict: native("url", url, strict))
                if actual != expected:
                    raise AssertionError((mode, level, reachable, actual, expected))
                rows.append(dict(opt_level=level, reachable_only=reachable, matched_calls=len(actual["guess_type_calls"])))
            report[mode] = dict(mime_database_file_hashes=db, wheel_sha256=wheels, source_sha256=sources,
                                calls_by_consumer=dict(Counter(c["call"] for c in expected["guess_type_calls"])),
                                result_sha256=sha256(json.dumps(expected, sort_keys=True).encode()).hexdigest(),
                                configurations=rows)
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
