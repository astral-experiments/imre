# MIME types for filenames and URLs

The bundled POSIX module uses CPython 3.14.5's complete built-in MIME registry. Module-level lookup also reads the host's existing `knownfiles` at runtime, in CPython's order. Each `MimeTypes()` instance starts with its own copy of the built-in tables. Its constructor accepts a list of custom UTF-8 table filenames.

```python
from mimetypes import MimeTypes, guess_file_type, guess_type

assert guess_type("https://example.org/files/archive.TGZ?download=1") == ("application/x-tar", "gzip")
assert guess_file_type("report.txt.gz") == ("text/plain", "gzip")
assert guess_type("data:image/svg+xml;charset=UTF-8,<svg/>") == ("image/svg+xml", None)

db = MimeTypes()
db.add_type("application/example", ".example", strict=False)
assert db.guess_file_type("file.example") == (None, None)
assert db.guess_file_type("file.example", strict=False) == ("application/example", None)
assert db.guess_extension("application/example", strict=False) == ".example"
```

`guess_type` parses URLs with a multi-character scheme. For those URLs the host, query and fragment do not provide a filename extension. `guess_file_type` treats its argument as a filesystem path. Both return a pair of MIME type and content encoding. Suffix aliases such as `.TGZ` ignore case, whereas `.gz` and `.Z` are case-sensitive encodings. The four pinned tables contain 198 strict mappings, nine common mappings, six suffix aliases and five encodings. [Source and license manifest](UPSTREAM.json)

For a hermetic module database, set `mimetypes.knownfiles = []` before calling `mimetypes.init()`. On first initialization even `init([])` reads the configured known files. After initialization, `init([])` keeps the current database, while `init()` creates a fresh one. A previous reference to `mimetypes.types_map` keeps the old dictionary. Native code supports qualified map reads and writes, reverse lookups, dotted or empty extensions in `add_type`, UTF-8 files, and `readfp` with `io.StringIO`. On the recorded Linux host, the union of mapping and encoding keys contains 1,581 extensions with system files and 215 with built-ins alone. The public URL and filename functions match ordinary CPython on 6,436 and 972 lookups respectively in each of ordinary and reachable-only O0/O3. The receipt identifies the actual system file by SHA-256. [API comparison](api.py), [registry and alias checks](configuration.py), [Linux results](api-linux.json)

The measured lookup inputs are strings, UTF-8 filename bytes and `pathlib.Path`. For `guess_file_type(b"\xff.txt")`, CPython returns `("text/plain", None)` and Imre raises `NotImplementedError` when decoding the surrogate-escaped filename. File-table reads accept string filenames. Arbitrary `os.PathLike` objects, other file-like objects for `readfp`, and mutable data imports such as `from mimetypes import types_map` are outside this slice. The native `MimeTypes` constructor also accepts explicit `None` as an empty file list, which CPython rejects. An undotted nonempty extension raises `NotImplementedError` in Imre. CPython warns and inserts it. Windows registry initialization and the module CLI are outside the POSIX implementation.

## Original Linux suite

Ordinary CPython 3.14.5 passes 27 of the 29 discovered occurrences and skips the two Windows cases. The unchanged source, data fixture and license are pinned by Git blob. Both optimization levels produced the following counts. [Reference](reference-linux.json), [per-occurrence roles and diagnostics](evidence-linux.json)

| Original test_mimetypes occurrences | O0 | O3 |
| --- | ---: | ---: |
| Native body, assertions, operation and fixture | 0 | 0 |
| Native body, assertions and operation with a mixed fixture | 7 | 7 |
| CPython body and assertions exercising native operations | 10 | 10 |
| Blocked | 10 | 10 |
| Original platform skip | 2 | 2 |
| Total | 29 | 29 |

The seven compiled methods cover case sensitivity, built-in data, byte paths, URL handling, adding types, reinitialization and stability. Six use a typed native equality assertion with an executable failing control. The stability method uses bundled `unittest`. CPython supplies the module hooks, and native code runs the original class setup with hermetic tables. Ten other unchanged methods run their original assertions in CPython against a native worker, including the UTF-8 `mime.types` fixture and mutation of the list returned by reverse lookup. [Runner](run.py)

Whole-suite native import reaches `from platform import win32_edition` on line 7 of the original test file. Isolating the original setup and teardown separately reaches the `global knownfiles` declaration. The ten blocked occurrences cover five CLI cases, `__all__`, lazy import inspection, the undotted-extension warning, the generic path-like fixture, and the file-reader test with a patched `open`. On Linux, `Win32MimeTypesTestCase.test_registry_accelerator` and `Win32MimeTypesTestCase.test_registry_parsing` each skip under the original “Windows only” class decorator. [Startup diagnostics](startup-linux.json), [unchanged test module](upstream/Lib/test/test_mimetypes.py)

## Werkzeug and Flask

The seven pinned wheels run on CPython with a native `guess_type` provider. Per registry configuration, the unchanged consumers perform 13 file uploads, 13 multipart encodes, 13 inline and 13 attachment `send_file` responses, and eight `SharedDataMiddleware` responses. All 60 MIME calls and their consumer outputs match the independent reference with the Linux system registry and again with the built-in registry, in all four native build configurations. On this host an uploaded `packet.cbor` has `application/cbor` with the system registry and `application/octet-stream` with built-ins. An inline `report.txt.gz` has `text/plain` and `Content-Encoding: gzip`. The attachment omits that encoding header. [Wheel comparison](werkzeug.py), [wheel and consumer hashes](werkzeug-linux.json)

The separate unchanged Flask gate serves all five requests on CPython. On the measured compiler all four native builds pass the MIME import, then stop at `werkzeug/datastructures/file_storage.py:8` importing `os.fsdecode`. They execute zero requests. [Full-application result](flask-linux.json)

## Reproduce

Use the pinned test-enabled CPython and a matched native compiler and runtime. The wheel comparison accepts the Flask gate's seven pinned wheels and an optional independently recorded `--reference` JSON for the same system database.

```sh
suite=benchmarks/cpython_stdlib/full/mimetypes
mkdir -p "$output"
"$python314" -IB benchmarks/cpython_stdlib/suite_inventory.py "$suite/UPSTREAM.json" --result "$output/reference-linux.json"
"$python314" -IB "$suite/run.py" --imre "$imre" --runtime "$runtime" --clang "$clang" --output "$output"
"$python314" -IB "$suite/api.py" --imre "$imre" --runtime "$runtime" --clang "$clang" --output "$output/api-linux.json"
"$python314" -IB "$suite/werkzeug.py" --imre "$imre" --runtime "$runtime" --clang "$clang" --wheelhouse "$wheelhouse" --output "$output/werkzeug-linux.json"
```
