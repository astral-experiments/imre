# Executable prototype contract

This is a deliberately restricted Python dialect for closed first-party programs. The current target is 64-bit Linux with LLVM/Clang 18. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

## Accepted source

| Area | Supported behavior |
| --- | --- |
| Modules | Closed first-party UTF-8 source graph; definitions and imports precede executable entry statements; optional module/function docstrings |
| Imports | Absolute, `from`, and package-relative imports; aliases and package re-exports; fixed module and definition bindings; no import cycles |
| Functions | Required parameter and return annotations; positional calls; direct, fixed bindings; no recursive call cycles |
| Types | `int`, `bool`, `str`, `None`, fixed nominal classes, concrete `list[T]`, fixed tuples such as `tuple[str, int]`, and `T \| None`; locals have one inferred or declared storage type |
| Classes | Declared fields, an explicit initializer, typed instance methods, alias-visible field mutation, and integer field augmented assignment |
| Statements | Single-target assignment, annotated assignment, integer augmented assignment, local string `+=`, `if`/`elif`/`else`, `while`, `for name in range(...)`, `break`, `continue`, `return`, `pass`, `assert condition` |
| Conditions | Explicit `bool`; `and`, `or`, `not`; short-circuit evaluation; `is None`/`is not None` with local flow narrowing |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`; unary `+`/`-`; all six value comparisons, including chains |
| Lists | One exact element type, including nested containers and classes; literals, shallow repetition, checked negative indexing, indexed assignment, `append(value)`, `pop()`, `len(...)`; alias-visible mutation |
| Tuples | Fixed length and a type per position; literals, bounded literal indexing, and `len(...)`; immutable slots |
| Strings | Immutable UTF-8 text, literals, concatenation, equality/inequality, Unicode scalar length, no-argument whitespace `split()`, input and output |
| Builtins | `print(value)` for one scalar; `input()` without a prompt; `int(int_or_bool_or_string)`; `len(string_or_list_or_tuple)`; `range` with one to three integer arguments inside a `for` loop |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. An empty list defaults to `list[int]` unless a typed storage, call, or return boundary supplies its element type. No annotation permits `Any`, a general union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Relar additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Relar's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

Mutable global values, first-class functions or methods, closures, decorators, reflection, dynamic attributes, generators, async, comprehensions, slicing, unpacking, keyword/default arguments, loop `else`, dictionaries, sets, mutable-list augmented assignment, and `is`/`is not` comparisons other than None checks are currently rejected. Conditions cannot rely on implicit truthiness. Unsupported operations are rejected even in dead branches and unused functions. A recursive call graph is rejected, including indirect recursion through methods or constructors; a future recursion feature needs an explicit stack policy.

## First-party module graphs

`relar check` and `relar build` resolve imports under one module root. By default this is the entry file's directory, or the parent of its outermost package when enclosing directories contain `__init__.py`. `--module-root DIRECTORY` selects the root explicitly. An entry inside a package follows `python -m package.program` semantics: enclosing package initializers are loaded and relative imports are valid. A package `__main__.py` can be the entry; `__init__.py` itself cannot.

For example, `import package.operations` binds `package`, `import package.operations as operations` binds the child module, and `from package.operations import solve as run` binds the same function under `run`. Module-qualified calls and imported calls resolve to their defining module's namespace. Function bodies can call a name imported later in that module, provided all definitions and imports precede executable statements. Module and definition bindings cannot be reassigned or shadowed by parameters or locals.

Imported modules, including every package `__init__.py`, may contain only definitions, imports, docstrings, and an optional `if __name__ == "__main__":` guard without an `else`. Every loaded module is type-checked and lowered for validation, including unused functions and inactive guards. Only the entry module's executable statements run. The fixed `__name__` string is `"__main__"` in the entry and the qualified module name in imported code.

Imports are resolved statically from `.py` files and packages with `__init__.py`. Wildcards, dynamic or function-local imports, namespace packages, import cycles, external dependencies, and standard-library imports or shadowing are rejected. A package can re-export definitions from its children; reading an unfinished parent's definitions through a circular import is rejected. Symlinks cannot escape the module root or give one source file multiple module identities. The source-string library APIs remain isolated; filesystem imports use the path-based APIs.

The I/O surface is intentionally small. `input()` reads a UTF-8 line and removes its trailing LF. A preceding CR remains part of the value, matching CPython's piped input on the Linux target. `int(str)` accepts signed ASCII decimal digits with optional separators between digits and surrounding whitespace. Other bases and Unicode decimal digits in input strings are outside this prototype's input format. Surrogate escapes in non-raw string literals are rejected: Ruff's analysis AST substitutes replacement characters for them, which is unsuitable for executable semantics. Strings support `+` and local `+=`; `text.split()` groups Python whitespace and omits empty fields, returning `list[str]`. Separators, split limits, other string methods, formatting, and arbitrary codecs are not implemented. CPython's configurable integer conversion digit limit is not reproduced.

## Fixed classes

A class has one nominal identity and a fixed set of declared fields. Import aliases and package re-exports preserve that identity; another class with identical fields remains a different type. Fields may store any supported scalar, concrete container, optional value, or another fixed class. Class names and module-qualified class names are accepted in annotations, including quoted type expressions. Quote forward references when the class is not yet defined, including a method's reference to its own class.

```python
class Counter:
    value: int

    def __init__(self, value: int) -> None:
        self.value = value

    def add(self, amount: int) -> int:
        self.value += amount
        return self.value
```

Class bodies contain field annotations without defaults, instance methods, and optional docstrings. Every method's first positional parameter is named `self`; its exact class type is inferred, or an explicit annotation must name that class. Other parameters and every return value require annotations. Method calls use the receiver's exact class and a fixed implementation. Passing or returning an instance preserves its identity, and aliases observe field replacement. `==` and `!=` between instances of the same class compare object identity.

Every class requires `__init__(...) -> None`, including classes with no fields. Its body contains only straight-line `self.field = expression` assignments, docstrings, and `pass`. Every declared field must be assigned exactly once. Assignment values may use constructor arguments and checked expressions, but cannot read or pass `self`; a partially initialized object never escapes construction. Local assignments, conditionals, loops, explicit returns, and direct later calls to `__init__` are rejected.

Inheritance, metaclasses, class variables, field defaults, properties, static/class methods, dynamically added fields, and special methods other than `__init__` are unsupported. Field and method names beginning with `__` are reserved, except the initializer. Class-field ownership types must form an acyclic graph, following list elements, tuple positions, and optional payloads. For example, fields `children: list["Node"]` and `parent: "Node | None"` inside class `Node` are rejected even when unused or initialized empty. Methods may accept or return their own class because those signatures do not create stored references.

## Concrete containers and optional values

`list[T]` has one concrete element type, such as `list[str]`, `list[Counter]`, or `list[list[int]]`. Types are invariant: an existing `list[str]` cannot become `list[str | None]` through assignment or a call. Every append and indexed write checks the exact element type. Repetition copies references to the same elements, so `[row] * 3` shares `row` across all three positions. `list[int]` retains its specialized tagged-integer storage and runtime calls; other lists use owned runtime slots.

Tuple types name each position, for example `tuple[str, int, list[Counter]]`; `tuple[()]` is empty. A tuple's slots cannot be reassigned, but a referenced list or instance can still be mutated. Indexes must be in-range integer literals, optionally negative. Variable indexes, computed indexes, variadic tuple annotations, tuple concatenation, and tuple repetition are rejected. List and tuple value comparisons are unsupported.

Literal elements receive types from an annotated variable, an existing storage location, a function or method argument, or a return annotation. This also applies inside nested literals and list repetition: `rows: list[list[str]] = [[], []]` creates two independent string lists. It does not convert an existing container to a different element type.

`T | None` permits a value of exactly `T` or `None`. Storage, call, and return boundaries explicitly wrap a present value; a bare return or falling off an optional-returning function produces `None`. No other union is supported. Use `value is None` or `value is not None` before accessing an unknown optional payload. Direct checks of local names narrow the relevant branches, short-circuit operands, assertions, and guarded loop bodies; early exits preserve facts on the remaining path. Rebinding updates those facts, and possible stores in a loop invalidate facts that later iterations could contradict.

Narrowing applies to local bindings, not mutable fields or indexed elements. Copy a field or element into a local before checking it; the local owns its value even if an alias later replaces the original slot. Compare an optional payload only after narrowing it. Neither an optional nor a container supplies implicit truthiness.

## Ownership and errors

Assignment and return expressions produce owned references when their type needs one. Operator and call operands may borrow a direct local: expressions cannot rebind a caller's slot in this closed subset. Parameter bindings that are never reassigned borrow their caller's root; other local slots own their values and release them on exit. A return acquires or transfers ownership even when returning a borrowed parameter. List-element reads, field and tuple reads, optional unwrapping, pops, and computed results remain owned across later evaluation, so mutation cannot invalidate an intermediate value. This also keeps a method receiver alive if later arguments replace its original field. An assignment publishes the replacement before dropping the old value. Ordinary subscript and field assignment evaluate the RHS first; augmented assignment instead captures the target and old value before evaluating its RHS.

Small signed 63-bit integers use tagged machine words. Checked arithmetic promotes to `num-bigint` when needed and can demote back to an immediate value. No integer-overflow assumption is handed to LLVM. Bounds and division-by-zero checks remain active at every optimization level. List storage never escapes the Rust runtime, and a read acquires an owned element reference before a later mutation can remove it.

Heap objects are nonmoving reference-counted allocations. Classes, tuples, generic lists, and present optional boxes own their stored values. An absent optional is a null handle; a present optional is a one-field box, including when its payload is an integer or another heap object. Writes acquire the replacement before releasing the displaced value, and reads acquire ownership before the receiver can be released. Mutable storage borrows end before displaced owners are dropped. The acyclic heap type graph prevents reference cycles through classes and containers. There are no finalizers or user callbacks under runtime borrows. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Runtime errors print an exception category and exit with status 1. There are no catchable exceptions or Python tracebacks yet. Normal return paths release references; fatal process-exit paths rely on process teardown. Allocation failures may terminate the process. `assert` is always enabled, independent of LLVM optimization.

Standard output uses bounded buffering for pipes and files, and flushes each printed line on a terminal. Pending output is also flushed before `input()` waits, before a reported runtime error exits, and before the generated process entry returns normally. If a write or flush returns an error, Relar reports `OSError`; fatal-error flushing is best effort and preserves the original exception category. Generated code explicitly flushes at process exit because its LLVM entry point does not run Rust's standard `main` lifecycle.

## Build and validate

Install the Rust toolchain specified in `rust-toolchain.toml`, Clang 18, and CPython 3.12 or later. Build the compiler and runtime together:

```sh
cargo build --workspace --release --locked
target/release/relar check tests/programs/ownership.py
target/release/relar build tests/programs/ownership.py -o /tmp/ownership --emit-llvm /tmp/ownership.ll
/tmp/ownership
python3 tests/differential.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/imports.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/classes.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/containers.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/io.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
```

`--runtime` selects a static library explicitly; otherwise it is found beside the compiler executable. `--clang` selects the Clang driver and `--opt-level` selects 0 through 3 (default 3). Every input module, executable, runtime, and saved LLVM output must have a distinct file identity; symlinks and hard links cannot bypass the input overwrite check.

`--sanitize` marks generated functions for AddressSanitizer and links its runtime. This instruments generated code; it does not retroactively instrument a prebuilt Rust static library. Instrumenting Rust runtime code requires a separate sanitizer build of that library. The test suites accept `--sanitize --opt-levels 1` to exercise this configuration, including leak checks on normal exits.

The differential suite compares full stdout with CPython at `-O0` and `-O3`, covering 260 integer pairs, aliases, ownership across calls and replacement, evaluation order, short-circuiting, loops, and runtime errors. Separate rejection cases cover type violations, ignored errors, unsupported dynamic operations, and recursive calls. These are correctness checks, not performance measurements.

The imports suite compares multi-file programs with CPython at `-O0` and `-O3`, including package entry points, aliases, re-exports, relative imports, late function-global lookup, and mutation across module boundaries. It also checks invalid imports, unused invalid code, source diagnostics, and protection of imported files from output writes.

The classes suite checks nominal types, imported class aliases, field replacement, temporary receivers, returns, method argument evaluation, and normal-exit ownership at `-O0` and `-O3`. Rejection cases cover incomplete construction, escaping `self`, invalid writes, dynamic class features, recursive calls, and cyclic field types.

The containers suite covers nested lists, tuple positions, optional ownership and local flow narrowing, contextual literals, shallow repetition, string concatenation, and whitespace splitting. It rejects invalid alias writes, unsafe optional accesses, and class cycles hidden inside containers.

The I/O suite checks complete output, large and Unicode writes, normal and fatal exit flushing, LF and CRLF pipe input, terminal line visibility, and output failures. Interactive checks use protocol handshakes and readiness deadlines rather than performance expectations. All five suites also run generated-code AddressSanitizer checks at `-O1` in CI.
