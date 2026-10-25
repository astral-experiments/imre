# Standard library support

Imre checks every name in the [CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib) top-level module registry (`sys.stdlib_module_names`) on Linux. The first tables cover 194 names without a leading underscore; [the appendix](#names-beginning-with-an-underscore) covers the other 103. Dotted APIs such as `os.path` appear under their parent.

**At a glance:** 64 of 297 modules accept an ordinary import; 2 more accept it only with `--reachable-only`. Across 32 modules with recorded suites, 92 complete test executions pass natively at both optimization levels; 12,443 of 12,590 upstream executions have a classification.

**Import** checks an otherwise empty file containing `import module`, in ordinary and reachable-only compilation. A successful import does not establish that any function works. Follow the evidence links for the supported operations and their limits.

**Native / upstream** counts unchanged test executions in the checked-in Linux suites: the body, assertions, operation and fixtures must all run on Imre at **both O0 and O3**. The denominator includes upstream skips, companion suites and repeated test IDs. A dash means there is no measurement; zero is a measured result. These are test counts, not percentages of an API.

The last two columns show where CPython still helps at O3. **CPython fixture**: 378 test bodies run on Imre with CPython providing part or all of the fixture (for example, setup or test objects). **CPython test**: 1,505 executions exercise Imre operations with CPython running the test body or checking its assertions. Neither count contributes to the native numerator.

## Modules with examples, accepted imports or suite results

| Module | Import | Native API example or boundary | Native / upstream | CPython fixture | CPython test |
| --- | --- | --- | ---: | ---: | ---: |
| `abc` | Yes | Abstract bases and `@abstractmethod` on fixed-layout classes. [Evidence](../tests/fixtures/classes/fixed_lineage.py) | — | — | — |
| `annotationlib` | Yes | `get_annotations`, `Format` and `type_repr` for supported declarations. [Evidence](../tests/fixtures/callables/annotations.py) | — | — | — |
| `argparse` | Yes | `ArgumentParser`, typed options, subcommands and generated help. [Evidence](../benchmarks/cpython_stdlib/full/argparse/README.md) | [0 / 1,893](../benchmarks/cpython_stdlib/full/argparse/) | 5 | 925 |
| `asyncio` | Yes | Run subprocess tasks, wait for a task group and handle cancellation. [Evidence](../tests/fixtures/processes/async_groups.py) | — | — | — |
| `base64` | Yes | `urlsafe_b64encode` on bytes. [Evidence](../benchmarks/cpython_stdlib/native_base64.py) | — | — | — |
| `builtins` | Yes | Read, assign and delete `builtins._`; escape supported representations with `ascii` and f-string `!a`. [Evidence](../benchmarks/cpython_stdlib/full/builtins/README.md) | [— / 147 (0 assessed)](../benchmarks/cpython_stdlib/full/builtins/) | — | — |
| `codecs` | Yes | `lookup`, codec names, stateless ASCII, Latin-1 and UTF-8 encoding and decoding, and byte-order marks. [Evidence](../benchmarks/cpython_stdlib/full/sys/BUILTINS.md) | — | — | — |
| `collections` | Yes | `Counter`, `deque`, `defaultdict` and selected `collections.abc.Mapping` operations. [Evidence](../benchmarks/cpython_stdlib/full/collections/README.md) | [8 / 659](../benchmarks/cpython_stdlib/full/collections/) | 8 | 14 |
| `configparser` | Reachable only | `ConfigParser` can be bound in unreachable code. Its constructor does not run natively. [Evidence](../crates/imre-compiler/src/modules/declarations.rs) | — | — | — |
| `contextlib` | Yes | `nullcontext` and selected single-yield `@contextmanager` functions. [Evidence](../tests/fixtures/processes/async_resource_arguments.py) | — | — | — |
| `contextvars` | Yes | Create and copy contexts, set and reset variable bindings, and isolate them in supported asyncio tasks. [Evidence](../benchmarks/cpython_stdlib/full/contextvars/README.md) | [2 / 56](../benchmarks/cpython_stdlib/full/contextvars/) | 0 | 12 |
| `copy` | Yes | Shallow and deep copies of supported containers and fixed-layout objects. [Evidence](../benchmarks/cpython_stdlib/full/copy/README.md) | [6 / 81](../benchmarks/cpython_stdlib/full/copy/) | 0 | 4 |
| `csv` | Yes | Read rows and apply supported CSV dialect options. [Evidence](../benchmarks/cpython_stdlib/full/csv/README.md) | [18 / 128](../benchmarks/cpython_stdlib/full/csv/) | 0 | 7 |
| `dataclasses` | Yes | Construct fixed-field dataclasses with defaults and factories. [Evidence](../tests/fixtures/dataclasses/record_factories.py) | — | — | — |
| `datetime` | Yes | Construct and compare `datetime.time` for timed logging; construct fixed-offset datetimes and timezones, and add timedeltas. [Evidence](../benchmarks/cpython_stdlib/full/datetime/README.md) | [0 / 1,098](../benchmarks/cpython_stdlib/full/datetime/) | 4 | 0 |
| `difflib` | Yes | `ndiff` on sequences of lines. [Evidence](../tests/fixtures/iterables/ndiff.py) | — | — | — |
| `doctest` | Yes | `DocTestSuite` and execution of selected original docstring examples. [Evidence](../benchmarks/cpython_stdlib/full/doctest/README.md) | — | — | — |
| `email` | Reachable only | Selected `message`, `parser` and `policy` declarations bind in unreachable code. [Evidence](../crates/imre-compiler/src/modules/declarations.rs) | — | — | — |
| `enum` | Yes | Named `Enum` and `StrEnum` members and aliases. [Evidence](../tests/fixtures/classes/ordinary_enums.py) | — | — | — |
| `errno` | Yes | Named errno constants, aliases and the `errorcode` registry. [Evidence](../benchmarks/cpython_stdlib/full/errno/README.md) | [3 / 3](../benchmarks/cpython_stdlib/full/errno/) | 0 | 0 |
| `fnmatch` | Yes | `fnmatch`, `fnmatchcase`, `filter`, `filterfalse` and `translate`. [Evidence](../benchmarks/cpython_stdlib/full/fnmatch/README.md) | [8 / 24](../benchmarks/cpython_stdlib/full/fnmatch/) | 0 | 16 |
| `functools` | Yes | Selected `partial`, caching decorators and dispatch operations. [Evidence](../benchmarks/cpython_stdlib/full/functools/EVIDENCE.md) | [0 / 329](../benchmarks/cpython_stdlib/full/functools/) | 9 | 17 |
| `genericpath` | Rejected | The genericpath companion suite is counted under `os`. [Evidence](../benchmarks/cpython_stdlib/full/os/README.md) | — | — | — |
| `gzip` | Yes | Read text lines and binary chunks from gzip files. [Evidence](../tests/fixtures/files/stream_reads.py) | — | — | — |
| `hashlib` | Yes | SHA-256 construction, incremental updates, digest and hexdigest. [Evidence](../benchmarks/cpython_stdlib/native_hashlib.py) | — | — | — |
| `importlib` | Yes | Construct and mutate `machinery.ModuleSpec`; with a configured cache tag, convert paths using `util.cache_from_source` and `util.source_from_cache`. [Evidence](../benchmarks/cpython_stdlib/full/importlib/README.md) | [0 / 1,346](../benchmarks/cpython_stdlib/full/importlib/) | 10 | 72 |
| `inspect` | Yes | `getattr_static` for selected owners and literal attribute names. [Evidence](../benchmarks/cpython_stdlib/full/inspect/README.md) | [0 / 362](../benchmarks/cpython_stdlib/full/inspect/) | 1 | 0 |
| `io` | Yes | In-memory `StringIO` and `BytesIO` streams. [Evidence](../benchmarks/cpython_stdlib/full/doctest/stringio/README.md) | — | — | — |
| `itertools` | Yes | Selected `chain`, `islice`, `repeat`, `tee` and `groupby` iterators. [Evidence](../benchmarks/cpython_stdlib/full/itertools/README.md) | [0 / 138](../benchmarks/cpython_stdlib/full/itertools/) | 11 | 0 |
| `json` | Yes | Decode and encode supported JSON values via `loads`, `load`, `dumps` and `dump`. [Evidence](../benchmarks/cpython_stdlib/full/json/README.md) | [0 / 216](../benchmarks/cpython_stdlib/full/json/) | 38 | 45 |
| `linecache` | Yes | Filesystem-backed `getline`, cache population and invalidation. [Evidence](../benchmarks/cpython_stdlib/full/linecache/README.md) | [0 / 29](../benchmarks/cpython_stdlib/full/linecache/) | 8 | 10 |
| `logging` | Yes | Loggers, formatters, configuration and file handlers with timed or size-based rotation. [Evidence](../benchmarks/cpython_stdlib/full/logging/README.md) | [0 / 272](../benchmarks/cpython_stdlib/full/logging/) | 16 | 43 |
| `math` | Yes | `prod` multiplies integer factors from iterables with an optional integer start. [Evidence](../tests/fixtures/iterables/math_product.py) | — | — | — |
| `operator` | Yes | `length_hint` for supported iterators. [Evidence](../benchmarks/cpython_stdlib/full/itertools/repeat.py) | — | — | — |
| `os` | Yes | File descriptors, process environment and POSIX `os.path`, including `abspath` and `normpath`. [Evidence](../benchmarks/cpython_stdlib/full/os/README.md) | [0 / 579](../benchmarks/cpython_stdlib/full/os/) | 27 | 15 |
| `pathlib` | Yes | Construct paths from segments or path-like objects; POSIX and Windows pure paths, traversal and selected file I/O. [Evidence](../benchmarks/cpython_stdlib/full/pathlib/README.md) | [0 / 1,369](../benchmarks/cpython_stdlib/full/pathlib/) | 136 | 63 |
| `posix` | Yes | Read and set supported POSIX user and group IDs; the companion suite is counted under `os`. [Evidence](../benchmarks/cpython_stdlib/full/os/CREDENTIALS.md) | — | — | — |
| `posixpath` | Yes | POSIX `os.path` operations on strings and bytes; path tests are counted under `os`. [Evidence](../tests/fixtures/paths/os_path.py) | — | — | — |
| `random` | Yes | Seed the shared generator and draw floats, bits, integer ranges, and choices from supported sequences. [Evidence](../benchmarks/cpython_stdlib/full/shutil/random.md) | — | — | — |
| `re` | Yes | Fixed regular expressions, captures and replacement escapes in `sub`. [Evidence](../benchmarks/cpython_stdlib/README.md) | — | — | — |
| `select` | Yes | Wait for readable or writable POSIX file descriptors with `poll` or `select`. [Evidence](../benchmarks/cpython_stdlib/full/select/operations.py) | [0 / 134](../benchmarks/cpython_stdlib/full/select/) | 7 | 6 |
| `selectors` | Yes | Use `SelectSelector` and `PollSelector` with `subprocess._PopenSelector` to choose how `Popen.communicate` waits for pipes. [Evidence](../tests/fixtures/processes/select_backend.py) | — | — | — |
| `shlex` | Yes | POSIX shell word splitting, quoting and joining. [Evidence](../benchmarks/cpython_stdlib/full/shlex/README.md) | [0 / 19](../benchmarks/cpython_stdlib/full/shlex/) | 2 | 3 |
| `shutil` | Yes | `which`, file and directory copies, removal and selected moves. [Evidence](../benchmarks/cpython_stdlib/full/shutil/README.md) | [0 / 223](../benchmarks/cpython_stdlib/full/shutil/) | 12 | 59 |
| `signal` | Yes | `SIGINT` and `SIGTERM` constants for supported event-loop handlers. [Evidence](../tests/fixtures/processes/async_gather_cancel.py) | — | — | — |
| `socket` | Yes | Bind an IPv4 TCP listener to port zero, accept a client, read bytes, and send a reply. [Evidence](../tests/fixtures/socket/listener.py) | [0 / 746](../benchmarks/cpython_stdlib/full/socket/) | 7 | 2 |
| `socketserver` | Yes | Handle one loopback connection with a synchronous TCPServer and a request-handler subclass. [Evidence](../tests/fixtures/socketserver/echo.py) | [0 / 27](../benchmarks/cpython_stdlib/full/socketserver/) | 2 | 1 |
| `stat` | Yes | POSIX mode constants and predicates, permission masks and `filemode`. [Evidence](../benchmarks/cpython_stdlib/full/stat/README.md) | [0 / 22](../benchmarks/cpython_stdlib/full/stat/) | 0 | 18 |
| `string` | Yes | Selected `Template` substitution, validation and identifier operations. [Evidence](../benchmarks/cpython_stdlib/full/string/README.md) | [15 / 54](../benchmarks/cpython_stdlib/full/string/) | 0 | 0 |
| `subprocess` | Yes | Launch with `Popen` and `run`, capture streams and handle deadlines. `getoutput` and `getstatusoutput` capture combined shell output. [Evidence](../benchmarks/cpython_stdlib/full/subprocess/README.md) | [0 / 353](../benchmarks/cpython_stdlib/full/subprocess/) | 54 | 97 |
| `sys` | Yes | Process arguments, streams, interpreter flags, and `sys.setrecursionlimit(2000)`. [Evidence](../benchmarks/cpython_stdlib/full/sys/README.md) | [0 / 97](../benchmarks/cpython_stdlib/full/sys/) | 6 | 3 |
| `sysconfig` | Yes | `get_path("scripts")`. [Evidence](../tests/fixtures/paths/executables.py) | — | — | — |
| `tempfile` | Yes | Scoped `TemporaryDirectory` workspaces. [Evidence](../tests/fixtures/workspaces/arguments.py) | — | — | — |
| `textwrap` | Yes | Dedent, indent, wrapping and shortening, including selected `TextWrapper` options. [Evidence](../benchmarks/cpython_stdlib/full/textwrap/README.md) | [15 / 68](../benchmarks/cpython_stdlib/full/textwrap/) | 0 | 53 |
| `time` | Yes | `sleep` and wall-clock time for supported process and logging paths. [Evidence](../tests/fixtures/processes/unobserved_child.py) | — | — | — |
| `tomllib` | Yes | Read selected TOML table keys with `load` on a binary stream. [Evidence](../tests/fixtures/paths/toml_discovery.py) | — | — | — |
| `types` | Yes | Fixed-layout `SimpleNamespace` construction and attributes. [Evidence](../benchmarks/cpython_stdlib/full/types/README.md) | [0 / 129](../benchmarks/cpython_stdlib/full/types/) | 3 | 0 |
| `typing` | Yes | Use selected annotations and aliases; register, look up and clear overloads for supported named functions. [Evidence](../benchmarks/cpython_stdlib/full/typing/README.md) | [0 / 712](../benchmarks/cpython_stdlib/full/typing/) | 3 | 2 |
| `unittest` | Yes | Selected `TestCase` assertions, fixtures, discovery and result handling. [Evidence](../benchmarks/cpython_stdlib/full/unittest/README.md) | [17 / 1,090](../benchmarks/cpython_stdlib/full/unittest/) | 9 | 12 |
| `urllib` | Yes | `urllib.parse.unquote` on strings with the default UTF-8 replacement behavior. [Evidence](../tests/fixtures/text/unquote.py) | — | — | — |
| `warnings` | Yes | Filter supported native warnings; format and display them with `formatwarning` and the default `showwarning`. [Evidence](../benchmarks/cpython_stdlib/full/warnings/README.md) | [0 / 187](../benchmarks/cpython_stdlib/full/warnings/) | 0 | 6 |
| `zipfile` | Yes | Read stored and deflated members and report CRC failures. [Evidence](../benchmarks/cpython_stdlib/native_zip_members.py) | — | — | — |
| `zlib` | Yes | `error` exception identity on compressed-input paths; this does not imply a general compression API. [Evidence](../tests/fixtures/files/stream_errors.py) | — | — | — |

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
| `codeop` |
| `colorsys` |
| `compileall` |
| `compression` |
| `concurrent` |
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
| `readline` |
| `reprlib` |
| `resource` |
| `rlcompleter` |
| `runpy` |
| `sched` |
| `secrets` |
| `shelve` |
| `site` |
| `smtplib` |
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

## Names beginning with an underscore

These include documented interfaces such as `__future__` and `_thread`, as well as implementation details. The same import and suite checks apply.

<details>
<summary>Show 103 module names</summary>

| Module | Import | Native API example or boundary | Native / upstream | CPython fixture | CPython test |
| --- | --- | --- | ---: | ---: | ---: |
| `__future__` | Yes | The `annotations` future statement and postponed annotations on supported classes and functions. [Evidence](../tests/fixtures/callables/annotations_future.py) | — | — | — |
| `_collections` | Yes | No API example recorded. | — | — | — |
| `_opcode` | Yes | `ENABLE_SPECIALIZATION` and `ENABLE_SPECIALIZATION_FT` are both zero for Imre’s native executables. [Evidence](../tests/fixtures/imports/opcode_flags.py) | — | — | — |
| `_string` | Yes | `formatter_parser` and `formatter_field_name_split` iterate format fields and their components. [Evidence](../tests/fixtures/text/formatter_ownership.py) | — | — | — |

Both import modes reject the following names. They have no recorded API example or suite measurement.

| Module |
| --- |
| `_abc` |
| `_aix_support` |
| `_android_support` |
| `_apple_support` |
| `_ast` |
| `_ast_unparse` |
| `_asyncio` |
| `_bisect` |
| `_blake2` |
| `_bz2` |
| `_codecs` |
| `_codecs_cn` |
| `_codecs_hk` |
| `_codecs_iso2022` |
| `_codecs_jp` |
| `_codecs_kr` |
| `_codecs_tw` |
| `_collections_abc` |
| `_colorize` |
| `_compat_pickle` |
| `_contextvars` |
| `_csv` |
| `_ctypes` |
| `_curses` |
| `_curses_panel` |
| `_datetime` |
| `_dbm` |
| `_decimal` |
| `_elementtree` |
| `_frozen_importlib` |
| `_frozen_importlib_external` |
| `_functools` |
| `_gdbm` |
| `_hashlib` |
| `_heapq` |
| `_hmac` |
| `_imp` |
| `_interpchannels` |
| `_interpqueues` |
| `_interpreters` |
| `_io` |
| `_ios_support` |
| `_json` |
| `_locale` |
| `_lsprof` |
| `_lzma` |
| `_markupbase` |
| `_md5` |
| `_multibytecodec` |
| `_multiprocessing` |
| `_opcode_metadata` |
| `_operator` |
| `_osx_support` |
| `_overlapped` |
| `_pickle` |
| `_posixshmem` |
| `_posixsubprocess` |
| `_py_abc` |
| `_py_warnings` |
| `_pydatetime` |
| `_pydecimal` |
| `_pyio` |
| `_pylong` |
| `_pyrepl` |
| `_queue` |
| `_random` |
| `_remote_debugging` |
| `_scproxy` |
| `_sha1` |
| `_sha2` |
| `_sha3` |
| `_signal` |
| `_sitebuiltins` |
| `_socket` |
| `_sqlite3` |
| `_sre` |
| `_ssl` |
| `_stat` |
| `_statistics` |
| `_strptime` |
| `_struct` |
| `_suggestions` |
| `_symtable` |
| `_sysconfig` |
| `_thread` |
| `_threading_local` |
| `_tkinter` |
| `_tokenize` |
| `_tracemalloc` |
| `_types` |
| `_typing` |
| `_uuid` |
| `_warnings` |
| `_weakref` |
| `_weakrefset` |
| `_winapi` |
| `_wmi` |
| `_zoneinfo` |
| `_zstd` |

</details>

The entry environment, `__main__`, is separate from CPython’s module registry. Imre’s `import __main__` probe: **Rejected**. The [entry-point guard](../tests/fixtures/module_values/entry_globals/main.py) uses `if __name__ == '__main__'`.

## Additional test subjects

These ledgers describe Python types or other subjects outside the top-level module inventory.

| Subject | Native / upstream | CPython fixture | CPython test |
| --- | ---: | ---: | ---: |
| `str` | [— / 138 (0 assessed)](../benchmarks/cpython_stdlib/full/str/) | — | — |

## Updating this page

Add an [API example](../benchmarks/cpython_stdlib/full/api_examples.toml) when a module gains a usable operation. Update the [per-test evidence](../benchmarks/cpython_stdlib/full/README.md) when native execution changes. From the repository root, use a current Linux Imre build and CPython 3.14.5:

```sh
python3.14 -B benchmarks/cpython_stdlib/full/support_table.py --imre target/release/imre
```

Pass `--check` to verify the checked-in page.
