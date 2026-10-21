# Executable prototype contract

This is a deliberately restricted Python dialect for closed first-party programs. The current target is 64-bit Linux with LLVM/Clang 18. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

## Accepted source

| Area | Supported behavior |
| --- | --- |
| Module | One UTF-8 source file; named functions precede executable statements; optional module/function docstrings |
| Functions | Required parameter and return annotations; positional calls; direct, fixed bindings; no recursive call cycles |
| Types | `int`, `bool`, `str`, `None`, `list[int]`; locals have one inferred or declared type |
| Statements | Single-target assignment, annotated assignment, integer augmented assignment, `if`/`elif`/`else`, `while`, `for name in range(...)`, `break`, `continue`, `return`, `pass`, `assert condition` |
| Conditions | Explicit `bool`; `and`, `or`, `not`; short-circuit evaluation |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`; unary `+`/`-`; all six value comparisons, including chains |
| Lists | Integer elements, including arbitrary-precision integers; literals, repetition, checked negative indexing, indexed assignment, `append(value)`, `pop()`, `len(...)`; alias-visible mutation |
| Strings | Immutable UTF-8 text, literals, equality/inequality, Unicode scalar length, input and output |
| Builtins | `print(value)` for one scalar; `input()` without a prompt; `int(int_or_bool_or_string)`; `len(string_or_list)`; `range` with one to three integer arguments inside a `for` loop |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. An empty list has type `list[int]`. No annotation permits `Any`, a union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Relar additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Relar's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

Function globals, first-class functions, closures, imports, classes, decorators, reflection, arbitrary attributes, generators, async, comprehensions, slicing, unpacking, keyword/default arguments, loop `else`, mutable-list augmented assignment, and identity comparisons are currently rejected. Conditions cannot rely on implicit truthiness. Unsupported operations are rejected even in dead branches and unused functions. A recursive call graph is rejected, including indirect recursion; a future recursion feature needs an explicit stack policy.

The I/O surface is intentionally small. `input()` reads a UTF-8 line and strips its LF or CRLF terminator. `int(str)` accepts signed ASCII decimal digits with optional separators between digits and surrounding whitespace. Other bases and Unicode decimal digits in input strings are outside this prototype's input format. Surrogate escapes in non-raw string literals are rejected: Ruff's analysis AST substitutes replacement characters for them, which is unsuitable for executable semantics. String methods, formatting, concatenation, and arbitrary codecs are not implemented. CPython's configurable integer conversion digit limit is not reproduced.

## Ownership and errors

Every IR expression produces an owned reference when its type needs one. A local read retains its value, a call borrows its evaluated arguments, a return transfers ownership, and function exit releases all local slots. Arguments and intermediate values stay rooted across other calls. An assignment publishes the replacement before dropping the old value. Ordinary subscript assignment evaluates the RHS first; augmented assignment instead captures the target and old item before evaluating its RHS.

Small signed 63-bit integers use tagged machine words. Checked arithmetic promotes to `num-bigint` when needed and can demote back to an immediate value. No integer-overflow assumption is handed to LLVM. Bounds and division-by-zero checks remain active at every optimization level. List storage never escapes the Rust runtime, and a read acquires an owned element reference before a later mutation can remove it.

Heap objects are nonmoving reference-counted allocations. Integer-only lists cannot form cycles. There are no finalizers or user callbacks under runtime borrows. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Runtime errors print an exception category and exit with status 1. There are no catchable exceptions or Python tracebacks yet. Normal return paths release references; fatal process-exit paths rely on process teardown. Allocation failures may terminate the process. `assert` is always enabled, independent of LLVM optimization.

## Build and validate

Install the Rust toolchain specified in `rust-toolchain.toml`, Clang 18, and CPython 3.12 or later. Build the compiler and runtime together:

```sh
cargo build --workspace --release --locked
target/release/relar check tests/programs/ownership.py
target/release/relar build tests/programs/ownership.py -o /tmp/ownership --emit-llvm /tmp/ownership.ll
/tmp/ownership
python3 tests/differential.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
```

`--runtime` selects a static library explicitly; otherwise it is found beside the compiler executable. `--clang` selects the Clang driver and `--opt-level` selects 0 through 3 (default 3). Source, executable, runtime, and saved LLVM output paths must be distinct.

`--sanitize` marks generated functions for AddressSanitizer and links its runtime. This instruments generated code; it does not retroactively instrument a prebuilt Rust static library. Instrumenting Rust runtime code requires a separate sanitizer build of that library. The differential suite accepts `--sanitize --opt-levels 1` to exercise this configuration, including leak checks on normal exits.

The differential suite compares full stdout with CPython at `-O0` and `-O3`, covering 260 integer pairs, aliases, ownership across calls and replacement, evaluation order, short-circuiting, loops, and runtime errors. Separate rejection cases cover type violations, ignored errors, unsupported dynamic operations, and recursive calls. These are correctness checks, not performance measurements.
