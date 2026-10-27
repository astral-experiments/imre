# POSIX stat

The bundled module comes from CPython 3.14.5's `Lib/stat.py`. The generator copies
its constant assignments, mode functions and permission table. It adds type
annotations, uses lists for the unequal-length groups in the table, and applies
the platform's unsigned conversion before inspecting a mode. Linux and macOS
have different mode widths; Darwin also has a whiteout type and two extra flag
masks. The normal module supplies CPython's accelerated behavior. The upstream
test also constructs a Python-only module; its probe intentionally omits those
accelerator overrides.

`UPSTREAM.json` pins the original module, C accelerator, license, and complete
`test_stat.py` by Git blob. The test driver runs **every discovered test** using
its original body, assertions, and filesystem setup and cleanup. It replaces the
two stat module objects with adapters to native executables, checks the result of
every requested function or constant, and records calls per test. Lookups of absent
exports are checked against the compiler. The native executables contain no
CPython dependency. The test driver itself uses CPython's unittest runner.

The Linux reference has 22 test IDs: 18 pass using native stat operations at
O0 and O3, four have their original platform skip, and zero entire test
methods run natively. The macOS job records its own skips and exercises the
Darwin accelerated constants. Separately, `check_modes.py` compares all 65,536
16-bit modes, including the ten file-type predicates, both masks and the text
from `filemode`. The operations runner also tests negative and large integers,
including exact overflow messages, against both upstream variants.

Reproduce using CPython 3.14.5 and its pinned `Lib/test`:

```sh
dir=benchmarks/cpython_stdlib/full/stat
python3 "$dir/generate_module.py" --check
python3 "$dir/run_operations.py" --imre target/release/imre \
  --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 \
  --cpython-source /path/to/cpython --result /tmp/stat.json
python3 "$dir/check_modes.py" --imre target/release/imre \
  --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18
python3 benchmarks/cpython_stdlib/suite_evidence.py "$dir/reference-linux.json" /tmp/stat.json
```

Use the platform Clang on macOS. Static integer expressions and direct boolean
arguments are supported. Calls through an explicitly typed integer callable
retain its integer argument contract. Runtime-loaded modules use the same
target-specific modes and accept objects implementing `__index__`. Their
callable adapters preserve binding when stored on a class. Frozen-loader
introspection is outside this surface.
