# Filesystem source lines

The pinned CPython 3.14.5 suite has 29 original test IDs. All 29 pass on
the Linux reference interpreter. At O0 and O3 the native evidence is:

| Original cases | Body and assertions | Product operation | Fixture |
|---:|---|---|---|
| 8 | Imre compiles the unchanged method; a native assertion checks every value | Imre | Original CPython instance lifecycle provides real files; a native instance receives their paths |
| 10 | Unchanged CPython method and TestCase assertions | Persistent Imre executable, including cache membership | CPython; the original invalidation setup populates the same native cache used during its test |
| 11 | Reference only | Unobserved | CPython |

The empty-file getline test makes zero calls. Loader callbacks, arbitrary
module mutation, private interpreter code objects, cache injection with a
different tuple layout, and free-threaded cache sharing are outside this
slice. No reference-only test counts as native.

Supported public operations include getline, getlines, updatecache, clearcache
and checkcache. The realized cache is an ordinary shared dictionary of
string keys and (integer size, floating mtime, mutable lines, full name)
tuples. Removing an entry does not invalidate a list already held by the
caller. getlines reads stale entries until the caller clears, updates or
checks the cache. Missing and undecodable files return fresh empty lists;
a successfully opened empty file caches one newline. A frozen filename can
use __file__ from a string-valued globals dictionary. lazycache returns false
on the supported filesystem-only inputs; a real get_source callback needs
additional callable support and is explicitly rejected.

Relative names try the filesystem, the resolved entry script directory, and
the startup PYTHONPATH in order. PYTHONSAFEPATH excludes the script directory;
an isolated command ignores its environment search path. Only deployed source
files can be read. Arbitrary sys.path edits, zip importers, installed CPython
source trees outside the configured paths and non-Unicode paths are outside
this native source-file profile.

The pinned tokenizer's first two binary lines select the encoding. Universal
newlines translate CR, LF and CRLF; other Unicode line separators remain in
the text. The portable registry covers UTF-8/BOM, ASCII, Latin-1, and 69
single-byte decoding tables. Unknown encodings yield an uncached empty list;
a registered unsupported text codec (e.g. shift_jis) raises NotImplementedError.
Non-text transforms raise CPython's LookupError. The 562 additional file inputs
cover every defined and undefined byte in the single-byte tables plus cookies,
BOMs, NULs and malformed input. These are controls outside the 29 cases.

Run the provenance and native checks with a test-enabled Linux CPython 3.14.5:

```sh
python benchmarks/cpython_stdlib/full/linecache/generate_codecs.py --check
python benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/linecache/UPSTREAM.json --result /tmp/linecache-reference.json
python benchmarks/cpython_stdlib/full/linecache/run.py \
  --cpython-source /path/to/cpython --imre /path/to/imre \
  --runtime /path/to/libimre_runtime.a --clang /path/to/clang --result /tmp/linecache-native.json
```
