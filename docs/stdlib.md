# Standard library support

Imre checks 194 publicly named top-level modules from [CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib) on Linux. Dotted APIs such as `os.path` appear under their parent. Private modules and third-party packages are outside this inventory.

**At a glance:** 51 modules accept an ordinary import; 3 more accept it only with `--reachable-only`. Across 25 public modules with recorded suites, 72 complete test executions pass natively at both optimization levels; 9,422 of 9,422 upstream executions have a classification.

**Import** checks an otherwise empty file containing `import module`, in ordinary and reachable-only compilation. A successful import does not establish that any function works. Follow the evidence links for the supported operations and their limits.

**Native / upstream** counts unchanged test executions in the checked-in Linux suites: the body, assertions, operation and fixtures must all run on Imre at **both O0 and O3**. The denominator includes upstream skips, companion suites and repeated test IDs. **Host-assisted** counts two other roles at O3: a native body with a CPython fixture, or a CPython test calling native operations. Mixed and blocked cases earn neither count. A dash means there is no measurement; zero is a measured result. These are test counts, not percentages of an API.

## Modules with examples, accepted imports or suite results

| Module | Import | Native API example or boundary | Native / upstream | Host-assisted |
| --- | --- | --- | ---: | ---: |
| `abc` | Yes | Abstract bases and `@abstractmethod` on fixed-layout classes. [Evidence](../tests/fixtures/classes/fixed_lineage.py) | — | — |
| `annotationlib` | Yes | `get_annotations`, `Format` and `type_repr` for supported declarations. [Evidence](../tests/fixtures/callables/annotations.py) | — | — |
| `argparse` | Yes | `ArgumentParser`, typed options, subcommands and generated help. [Evidence](../benchmarks/cpython_stdlib/full/argparse/README.md) | [0 / 1,893](../benchmarks/cpython_stdlib/full/argparse/) | 930 |
| `asyncio` | Yes | Run subprocess tasks, wait for a task group and handle cancellation. [Evidence](../tests/fixtures/processes/async_groups.py) | — | — |
| `base64` | Yes | `urlsafe_b64encode` on bytes. [Evidence](../benchmarks/cpython_stdlib/native_base64.py) | — | — |
| `builtins` | Yes | Read, assign and delete `builtins._`, including the value saved by the default display hook. [Evidence](../benchmarks/cpython_stdlib/full/sys/BUILTINS.md) | — | — |
| `collections` | Yes | `Counter`, `deque`, `defaultdict` and selected `collections.abc.Mapping` operations. [Evidence](../benchmarks/cpython_stdlib/full/collections/README.md) | [0 / 659](../benchmarks/cpython_stdlib/full/collections/) | 11 |
| `configparser` | Reachable only | `ConfigParser` can be bound in unreachable code. Its constructor does not run natively. [Evidence](../crates/imre-compiler/src/modules/declarations.rs) | — | — |
| `contextlib` | Yes | `nullcontext` and selected single-yield `@contextmanager` functions. [Evidence](../tests/fixtures/processes/async_resource_arguments.py) | — | — |
| `copy` | Yes | Shallow and deep copies of supported containers and fixed-layout objects. [Evidence](../benchmarks/cpython_stdlib/full/copy/README.md) | [6 / 81](../benchmarks/cpython_stdlib/full/copy/) | 4 |
| `csv` | Yes | Read rows and apply supported CSV dialect options. [Evidence](../benchmarks/cpython_stdlib/full/csv/README.md) | [18 / 128](../benchmarks/cpython_stdlib/full/csv/) | 7 |
| `dataclasses` | Yes | Construct fixed-field dataclasses with defaults and factories. [Evidence](../tests/fixtures/dataclasses/record_factories.py) | — | — |
| `datetime` | Yes | Construct and compare `datetime.time` for timed logging; construct fixed-offset datetimes and timezones, and add timedeltas. [Evidence](../benchmarks/cpython_stdlib/full/datetime/README.md) | [0 / 1,098](../benchmarks/cpython_stdlib/full/datetime/) | 4 |
| `difflib` | Yes | `ndiff` on sequences of lines. [Evidence](../tests/fixtures/iterables/ndiff.py) | — | — |
| `doctest` | Yes | `DocTestSuite` and execution of selected original docstring examples. [Evidence](../benchmarks/cpython_stdlib/full/doctest/README.md) | — | — |
| `email` | Reachable only | Selected `message`, `parser` and `policy` declarations bind in unreachable code. [Evidence](../crates/imre-compiler/src/modules/declarations.rs) | — | — |
| `enum` | Yes | Named `Enum` and `StrEnum` members and aliases. [Evidence](../tests/fixtures/classes/ordinary_enums.py) | — | — |
| `errno` | Yes | Named errno constants, aliases and the `errorcode` registry. [Evidence](../benchmarks/cpython_stdlib/full/errno/README.md) | [3 / 3](../benchmarks/cpython_stdlib/full/errno/) | 0 |
| `fnmatch` | Yes | `fnmatch`, `fnmatchcase`, `filter`, `filterfalse` and `translate`. [Evidence](../benchmarks/cpython_stdlib/full/fnmatch/README.md) | [8 / 24](../benchmarks/cpython_stdlib/full/fnmatch/) | 16 |
| `functools` | Yes | Selected `partial`, caching decorators and dispatch operations. [Evidence](../benchmarks/cpython_stdlib/full/functools/EVIDENCE.md) | [0 / 329](../benchmarks/cpython_stdlib/full/functools/) | 19 |
| `genericpath` | Rejected | The genericpath companion suite is counted under `os`. [Evidence](../benchmarks/cpython_stdlib/full/os/README.md) | — | — |
| `gzip` | Yes | Read text lines and binary chunks from gzip files. [Evidence](../tests/fixtures/files/stream_reads.py) | — | — |
| `hashlib` | Yes | SHA-256 construction, incremental updates, digest and hexdigest. [Evidence](../benchmarks/cpython_stdlib/native_hashlib.py) | — | — |
| `importlib` | Reachable only | `importlib.resources.files` binds as a declaration in unreachable code. [Evidence](../crates/imre-compiler/src/modules/declarations.rs) | — | — |
| `inspect` | Yes | `getattr_static` for selected owners and literal attribute names. [Evidence](../benchmarks/cpython_stdlib/full/inspect/README.md) | [0 / 362](../benchmarks/cpython_stdlib/full/inspect/) | 1 |
| `io` | Yes | In-memory `StringIO` and `BytesIO` streams. [Evidence](../benchmarks/cpython_stdlib/full/doctest/stringio/README.md) | — | — |
| `itertools` | Yes | Selected `chain`, `islice`, `repeat`, `tee` and `groupby` iterators. [Evidence](../benchmarks/cpython_stdlib/full/itertools/README.md) | [0 / 138](../benchmarks/cpython_stdlib/full/itertools/) | 11 |
| `json` | Yes | Decode and encode supported JSON values via `loads`, `load`, `dumps` and `dump`. [Evidence](../benchmarks/cpython_stdlib/full/json/README.md) | [0 / 216](../benchmarks/cpython_stdlib/full/json/) | 83 |
| `linecache` | Yes | Filesystem-backed `getline`, cache population and invalidation. [Evidence](../benchmarks/cpython_stdlib/full/linecache/README.md) | [0 / 29](../benchmarks/cpython_stdlib/full/linecache/) | 18 |
| `logging` | Yes | Loggers, formatters, configuration and file handlers with timed or size-based rotation. [Evidence](../benchmarks/cpython_stdlib/full/logging/README.md) | [0 / 272](../benchmarks/cpython_stdlib/full/logging/) | 58 |
| `operator` | Yes | `length_hint` for supported iterators. [Evidence](../benchmarks/cpython_stdlib/full/itertools/repeat.py) | — | — |
| `os` | Yes | File descriptors, process environment and POSIX `os.path`, including `abspath` and `normpath`. [Evidence](../benchmarks/cpython_stdlib/full/os/README.md) | [0 / 579](../benchmarks/cpython_stdlib/full/os/) | 41 |
| `pathlib` | Yes | Construct paths from segments or path-like objects; POSIX and Windows pure paths, traversal and selected file I/O. [Evidence](../benchmarks/cpython_stdlib/full/pathlib/README.md) | [0 / 1,369](../benchmarks/cpython_stdlib/full/pathlib/) | 197 |
| `posix` | Yes | Read and set supported POSIX user and group IDs; the companion suite is counted under `os`. [Evidence](../benchmarks/cpython_stdlib/full/os/CREDENTIALS.md) | — | — |
| `posixpath` | Yes | POSIX `os.path` operations on strings and bytes; path tests are counted under `os`. [Evidence](../tests/fixtures/paths/os_path.py) | — | — |
| `re` | Yes | Fixed regular expressions, captures and replacement escapes in `sub`. [Evidence](../benchmarks/cpython_stdlib/README.md) | — | — |
| `shlex` | Yes | POSIX shell word splitting, quoting and joining. [Evidence](../benchmarks/cpython_stdlib/full/shlex/README.md) | [0 / 19](../benchmarks/cpython_stdlib/full/shlex/) | 5 |
| `shutil` | Yes | `which`, file and directory copies, removal and selected moves. [Evidence](../benchmarks/cpython_stdlib/full/shutil/README.md) | [0 / 223](../benchmarks/cpython_stdlib/full/shutil/) | 71 |
| `signal` | Yes | `SIGINT` and `SIGTERM` constants for supported event-loop handlers. [Evidence](../tests/fixtures/processes/async_gather_cancel.py) | — | — |
| `stat` | Yes | POSIX mode constants and predicates, permission masks and `filemode`. [Evidence](../benchmarks/cpython_stdlib/full/stat/README.md) | [0 / 22](../benchmarks/cpython_stdlib/full/stat/) | 18 |
| `string` | Yes | Selected `Template` substitution, validation and identifier operations. [Evidence](../benchmarks/cpython_stdlib/full/string/README.md) | [15 / 54](../benchmarks/cpython_stdlib/full/string/) | 0 |
| `subprocess` | Yes | Selected `Popen` and `run` calls, captured streams, deadlines and errors. [Evidence](../benchmarks/cpython_stdlib/full/subprocess/README.md) | [0 / 353](../benchmarks/cpython_stdlib/full/subprocess/) | 97 |
| `sys` | Yes | Process arguments, streams, interpreter flags and selected attributes. [Evidence](../benchmarks/cpython_stdlib/full/sys/README.md) | [0 / 97](../benchmarks/cpython_stdlib/full/sys/) | 5 |
| `sysconfig` | Yes | `get_path("scripts")`. [Evidence](../tests/fixtures/paths/executables.py) | — | — |
| `tempfile` | Yes | Scoped `TemporaryDirectory` workspaces. [Evidence](../tests/fixtures/workspaces/arguments.py) | — | — |
| `textwrap` | Yes | Dedent, indent, wrapping and shortening, including selected `TextWrapper` options. [Evidence](../benchmarks/cpython_stdlib/full/textwrap/README.md) | [15 / 68](../benchmarks/cpython_stdlib/full/textwrap/) | 53 |
| `time` | Yes | `sleep` and wall-clock time for supported process and logging paths. [Evidence](../tests/fixtures/processes/unobserved_child.py) | — | — |
| `tomllib` | Yes | Read selected TOML table keys with `load` on a binary stream. [Evidence](../tests/fixtures/paths/toml_discovery.py) | — | — |
| `types` | Yes | Fixed-layout `SimpleNamespace` construction and attributes. [Evidence](../benchmarks/cpython_stdlib/full/types/README.md) | [0 / 129](../benchmarks/cpython_stdlib/full/types/) | 3 |
| `typing` | Yes | Selected annotations and aliases, including `Callable`, `Literal`, `Self` and `Union`. [Evidence](../docs/subset.md) | — | — |
| `unittest` | Yes | Selected `TestCase` assertions, fixtures, discovery and result handling. [Evidence](../benchmarks/cpython_stdlib/full/unittest/README.md) | [7 / 1,090](../benchmarks/cpython_stdlib/full/unittest/) | 17 |
| `urllib` | Yes | `urllib.parse.unquote` on strings with the default UTF-8 replacement behavior. [Evidence](../tests/fixtures/text/unquote.py) | — | — |
| `warnings` | Yes | Warning filters and matching for supported native warnings. [Evidence](../benchmarks/cpython_stdlib/full/warnings/README.md) | [0 / 187](../benchmarks/cpython_stdlib/full/warnings/) | 2 |
| `zipfile` | Yes | Read stored and deflated members and report CRC failures. [Evidence](../benchmarks/cpython_stdlib/native_zip_members.py) | — | — |
| `zlib` | Yes | `error` exception identity on compressed-input paths; this does not imply a general compression API. [Evidence](../tests/fixtures/files/stream_errors.py) | — | — |

## Modules without an accepted import or a suite measurement

Both import probes reject these names. No native API example or full-suite ledger is recorded here.

| Module |
| --- |
| `array` |
| `ast` |
| `atexit` |
| `bdb` |
| `binascii` |
| `bisect` |
| `bz2` |
| `cProfile` |
| `calendar` |
| `cmath` |
| `cmd` |
| `code` |
| `codecs` |
| `codeop` |
| `colorsys` |
| `compileall` |
| `compression` |
| `concurrent` |
| `contextvars` |
| `copyreg` |
| `ctypes` |
| `curses` |
| `dbm` |
| `decimal` |
| `dis` |
| `encodings` |
| `ensurepip` |
| `faulthandler` |
| `fcntl` |
| `filecmp` |
| `fileinput` |
| `fractions` |
| `ftplib` |
| `gc` |
| `getopt` |
| `getpass` |
| `gettext` |
| `glob` |
| `graphlib` |
| `grp` |
| `heapq` |
| `hmac` |
| `html` |
| `http` |
| `imaplib` |
| `ipaddress` |
| `keyword` |
| `locale` |
| `lzma` |
| `mailbox` |
| `marshal` |
| `math` |
| `mimetypes` |
| `mmap` |
| `modulefinder` |
| `msvcrt` |
| `multiprocessing` |
| `netrc` |
| `nt` |
| `ntpath` |
| `nturl2path` |
| `numbers` |
| `opcode` |
| `optparse` |
| `pdb` |
| `pickle` |
| `pickletools` |
| `pkgutil` |
| `platform` |
| `plistlib` |
| `poplib` |
| `pprint` |
| `profile` |
| `pstats` |
| `pty` |
| `pwd` |
| `py_compile` |
| `pyclbr` |
| `pydoc` |
| `pydoc_data` |
| `pyexpat` |
| `queue` |
| `quopri` |
| `random` |
| `readline` |
| `reprlib` |
| `resource` |
| `rlcompleter` |
| `runpy` |
| `sched` |
| `secrets` |
| `select` |
| `selectors` |
| `shelve` |
| `site` |
| `smtplib` |
| `socket` |
| `socketserver` |
| `sqlite3` |
| `sre_compile` |
| `sre_constants` |
| `sre_parse` |
| `ssl` |
| `statistics` |
| `stringprep` |
| `struct` |
| `symtable` |
| `syslog` |
| `tabnanny` |
| `tarfile` |
| `termios` |
| `threading` |
| `timeit` |
| `token` |
| `tokenize` |
| `trace` |
| `traceback` |
| `tracemalloc` |
| `tty` |
| `unicodedata` |
| `uuid` |
| `venv` |
| `wave` |
| `weakref` |
| `webbrowser` |
| `winreg` |
| `wsgiref` |
| `xml` |
| `xmlrpc` |
| `zipapp` |
| `zipimport` |
| `zoneinfo` |

## Proposed scope exclusions

These names are still probed. The reasons come from the [scope inventory](../benchmarks/cpython_stdlib/full/scope.toml); they do not exempt any tests in a measured suite.

| Module | Import | Reason |
| --- | --- | --- |
| `antigravity` | Rejected | Easter egg that launches a browser |
| `idlelib` | Rejected | IDLE desktop IDE |
| `this` | Rejected | Easter egg that prints the Zen of Python |
| `tkinter` | Rejected | Optional Tcl/Tk desktop UI |
| `turtle` | Rejected | Educational desktop graphics |
| `turtledemo` | Rejected | Interactive turtle demo programs |
| `winsound` | Rejected | Optional Windows sound playback |

## Additional test subjects

These ledgers describe Python types or other subjects outside the top-level module inventory.

| Subject | Native / upstream | Host-assisted |
| --- | ---: | ---: |
| `str` | [— / 138 (0 assessed)](../benchmarks/cpython_stdlib/full/str/) | — |

## Updating this page

Add an [API example](../benchmarks/cpython_stdlib/full/api_examples.toml) when a module gains a usable operation. Update the [per-test evidence](../benchmarks/cpython_stdlib/full/README.md) when native execution changes. From the repository root, use a current Linux Imre build and CPython 3.14.5:

```sh
python3.14 -B benchmarks/cpython_stdlib/full/support_table.py --imre target/release/imre
```

Pass `--check` to verify the checked-in page.
