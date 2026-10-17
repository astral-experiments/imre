# Executable prototype contract

This is a deliberately restricted Python dialect for closed first-party programs. The current target is 64-bit Linux with LLVM/Clang 18. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

## Accepted source

| Area | Supported behavior |
| --- | --- |
| Modules | Closed first-party UTF-8 source graph; definitions and imports precede executable entry statements; optional module/function docstrings |
| Imports | Absolute, `from`, and package-relative imports; aliases and package re-exports; fixed module and definition bindings; no import cycles |
| Functions | Required parameter and return annotations; positional calls; direct, fixed bindings; no recursive call cycles |
| Types | `int`, `bool`, `str`, `None`, fixed nominal classes, concrete `list[T]` and `dict[K, V]`, fixed tuples such as `tuple[str, int]`, and `T \| None`; locals have one inferred or declared storage type |
| Classes | Declared fields, an explicit initializer, typed instance methods, alias-visible field mutation, and integer field augmented assignment |
| Statements | Single-target and fixed-tuple unpacking assignment, annotated assignment, integer augmented assignment, local string `+=`, dictionary entry deletion, `if`/`elif`/`else`, `while`, `for` over ranges or concrete containers, `break`, `continue`, `return`, `pass`, `assert condition` |
| Conditions | Explicit `bool`; `and`, `or`, `not`; short-circuit evaluation; `is None`/`is not None` with local flow narrowing |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`; unary `+`/`-`; all six value comparisons, including chains |
| Lists | One exact element type, including nested containers and classes; literals, shallow repetition, checked negative indexing, indexed assignment, iteration, `append(value)`, `pop()`, `len(...)`; alias-visible mutation |
| Dictionaries | Exact `int` or `str` keys and one concrete value type; literals, contextual `dict()`, lookup/write/delete, integer augmented writes, `in`/`not in`, `get`, `len`, and insertion-ordered key/items iteration |
| Tuples | Fixed length and a type per position; literals, bounded literal indexing, unpacking into names, and `len(...)`; immutable slots |
| Strings | Immutable UTF-8 text, literals, concatenation, equality/inequality, Unicode scalar length, no-argument whitespace `split()`, input and output |
| Builtins | `print(value)` for one scalar; `input()` without a prompt; `int(int_or_bool_or_string)`; `len(string_or_container)`; contextual empty `dict()`; `range` with one to three integer arguments and `enumerate(container[, start])` inside a `for` loop |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. An empty list defaults to `list[int]` unless a typed storage, call, or return boundary supplies its element type. No annotation permits `Any`, a general union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Relar additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Relar's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

Mutable global values, first-class functions or methods, closures, decorators, reflection, dynamic attributes, generators, async, comprehensions, slicing, starred or variable-length unpacking, keyword/default arguments, loop `else`, sets, mutable-list augmented assignment, and `is`/`is not` comparisons other than None checks are currently rejected. Conditions cannot rely on implicit truthiness. Unsupported operations are rejected even in dead branches and unused functions. A recursive call graph is rejected, including indirect recursion through methods or constructors; a future recursion feature needs an explicit stack policy.

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

Inheritance, metaclasses, class variables, field defaults, properties, static/class methods, dynamically added fields, and special methods other than `__init__` are unsupported. Field and method names beginning with `__` are reserved, except the initializer. Class-field ownership types must form an acyclic graph, following list elements, dictionary values, tuple positions, and optional payloads. For example, fields `children: list["Node"]`, `named: dict[str, "Node"]`, and `parent: "Node | None"` inside class `Node` are rejected even when unused or initialized empty. Methods may accept or return their own class because those signatures do not create stored references.

## Concrete containers and optional values

`list[T]` has one concrete element type, such as `list[str]`, `list[Counter]`, or `list[list[int]]`. Types are invariant: an existing `list[str]` cannot become `list[str | None]` through assignment or a call. Every append and indexed write checks the exact element type. Repetition copies references to the same elements, so `[row] * 3` shares `row` across all three positions. `list[int]` retains its specialized tagged-integer storage and runtime calls; other lists use owned runtime slots.

`dict[K, V]` requires exactly `int` or `str` keys and one concrete value type. Keys use numeric or exact Unicode string equality; booleans cannot stand in for integer keys. Nonempty literals infer these types from their first entry; `{}` and `dict()` require a concrete contextual type. Duplicate keys keep the first insertion position and the last value. Replacing a value keeps its position, while deletion and reinsertion move the key to the end. `mapping[key]`, assignment, `del mapping[key]`, integer augmented assignment, `len`, and single `in`/`not in` comparisons are supported. Missing required reads and deletes report `KeyError`.

`mapping.get(key)` returns `V | None`, flattened when `V` is already optional or `None`. A default must fit `V | None`; a guaranteed `V` default gives a `V` result. Defaults are evaluated eagerly even when the key exists. Container literals in defaults receive the dictionary's declared value type. Dictionaries are invariant, and value comparisons, expansion with `**`, copy/iterable constructors, `keys`, `values`, and other mapping methods are not implemented.

Tuple types name each position, for example `tuple[str, int, list[Counter]]`; `tuple[()]` is empty. A tuple's slots cannot be reassigned, but a referenced list or instance can still be mutated. Indexes must be in-range integer literals, optionally negative. Variable indexes, computed indexes, variadic tuple annotations, tuple concatenation, and tuple repetition are rejected. List and tuple value comparisons are unsupported.

Fixed tuples can be unpacked into names in assignments and loop targets, including nested tuple or list target syntax: `name, (count, values) = record`. Every source value is evaluated before any target is rebound, so swaps and repeated target names retain Python's assignment order. Targets cannot contain stars, attributes, or subscripts, and a list value cannot be unpacked because its length is not part of its type.

`for value in values` evaluates a concrete list once and retains that original list even if the source name or field is later replaced. It reads the current length and acquires the next element at each step. Appending, removing, or replacing elements through an alias therefore has Python's index-based list-iteration behavior. The current item remains owned if the body removes it from the list. `enumerate(values, start)` captures the list before evaluating its optional integer start and yields fixed `(int, T)` tuples; its counter supports arbitrary precision and is independent of visible target rebinding. Both forms support `break`, `continue`, nested loops, and early returns. A target first introduced by a loop remains potentially uninitialized afterward because the loop may be empty. Iterator values, tuple/string iteration, and user-defined iteration protocols are not supported.

`for key in mapping` and `for key, value in mapping.items()` visit insertion order. `enumerate` accepts either form and creates the iterator after evaluating its start argument. Each yielded key/value has its own owner before the body runs. Replacing an existing value is allowed and later iterations see the replacement. A changed size at the next step reports `RuntimeError`, including after the last yielded item. Same-size structural mutation does not provide a portable traversal guarantee; it may report changed keys or skip entries. `items()` is supported only at a loop boundary, including inside loop-head `enumerate`; views and iterators cannot be stored as user values.

Literal elements receive types from an annotated variable, an existing storage location, a function or method argument, or a return annotation. This also applies inside nested literals and list repetition: `rows: list[list[str]] = [[], []]` creates two independent string lists. It does not convert an existing container to a different element type.

`T | None` permits a value of exactly `T` or `None`. Storage, call, and return boundaries explicitly wrap a present value; a bare return or falling off an optional-returning function produces `None`. No other union is supported. Use `value is None` or `value is not None` before accessing an unknown optional payload. Direct checks of local names narrow the relevant branches, short-circuit operands, assertions, and guarded loop bodies; early exits preserve facts on the remaining path. Rebinding updates those facts, and possible stores in a loop invalidate facts that later iterations could contradict.

Narrowing applies to local bindings, not mutable fields or indexed elements. Copy a field or element into a local before checking it; the local owns its value even if an alias later replaces the original slot. Compare an optional payload only after narrowing it. Neither an optional nor a container supplies implicit truthiness.

## Ownership and errors

Assignment and return expressions produce owned references when their type needs one. Operator and call operands may borrow a direct local: expressions cannot rebind a caller's slot in this closed subset. Parameter bindings that are never reassigned borrow their caller's root; other local slots own their values and release them on exit. A return acquires or transfers ownership even when returning a borrowed parameter. List-element reads generally remain owned across later evaluation; field and tuple reads, optional unwrapping, pops, and computed results are also owned, so mutation cannot invalidate an intermediate value. This also keeps a method receiver alive if later arguments replace its original field. An assignment publishes the replacement before dropping the old value. Ordinary subscript and field assignment evaluate the RHS first; augmented assignment instead captures the target and old value before evaluating its RHS.

Integer representation is canonical: values from `-2**62` through `2**62 - 1` always use tagged machine words; values outside that range use `num-bigint`. Checked arithmetic promotes and demotes as needed. Foreign adapters must preserve this invariant before passing integers to generated code.

Conservative interval facts track immutable integer values in local slots across reassignment and control flow. Heap reads and user-call results start unknown. Direct arithmetic requires proofs that both operands and the result fit the small representation; only proved representable arithmetic and tagging shifts receive LLVM's `nsw` overflow flag. Unknown bounds, analysis overflow, or a possibly zero divisor retain checked lowering. Division by zero remains a language error at every optimization level. `range` arguments are captured once, and a specialized hidden cursor must also fit after its final increment; assigning to the visible target does not change its progression.

Old-value and exit ownership checks can be omitted for an integer local only when every possible write or move into that slot is proved small. Parameters and unknown replacements retain those checks.

A proved nonnegative small index may use a decoded native-index ABI, but each read or write still checks the container's current bounds. Calls, alias mutation, and resizing cannot invalidate a cached length because no such length proof is used. Negative or arbitrary-precision indices retain their normal normalization and error behavior. Element reads acquire ownership before later evaluation can remove them. The narrow exception is a direct integer-list read in a two-operand integer comparison whose complete operand trees are proved nonmutating; the local keeps the list rooted until the comparison consumes the borrowed handle. Chained comparisons and potentially mutating evaluation retain owned reads. A same-list copy with simple, proved indices can also combine its get and set, checking and acquiring the source before checking and replacing the destination.

Heap objects are nonmoving reference-counted allocations. Classes, tuples, generic lists, dictionaries, and present optional boxes own their stored values. An absent optional is a null handle; a present optional is a one-field box, including when its payload is an integer or another heap object. Writes acquire the replacement before releasing the displaced value, and reads acquire ownership before the receiver can be released, apart from the rooted comparison borrow above. Storage access uses scoped `UnsafeCell` closures on one execution thread: mutable access is exclusive, operations cannot reenter through aliases, and no storage reference or buffer address may escape. Independently owned values and explicitly constrained borrowed integer handles may escape; displaced owners are dropped after access ends. Dictionaries use a randomized hash index and insertion-ordered entries. Internal iterators root their dictionary and prevent entry compaction until released, but keep no storage reference across a loop body. The acyclic heap type graph prevents reference cycles through classes and containers. There are no finalizers or user callbacks under storage access. Threaded or reentrant extension boundaries would need to preserve these rules. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Runtime errors print an exception category and exit with status 1. There are no catchable exceptions or Python tracebacks yet. Normal return paths release references; fatal process-exit paths rely on process teardown. Allocation failures may terminate the process. `assert` is always enabled, independent of LLVM optimization.

Standard output uses bounded buffering for pipes and files, and flushes each printed line on a terminal. Pending output is also flushed before `input()` waits, before a reported runtime error exits, and before the generated process entry returns normally. If a write or flush returns an error, Relar reports `OSError`; fatal-error flushing is best effort and preserves the original exception category. Generated code explicitly flushes at process exit because its LLVM entry point does not run Rust's standard `main` lifecycle.

Signal handling remains native. Relar does not install CPython's signal handlers, so a closed output pipe can terminate the process with `SIGPIPE` before an I/O error is returned. This behavior predates output buffering.

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

`--runtime` selects a static library explicitly; otherwise it is found beside the compiler executable. `--clang` selects the Clang driver and `--opt-level` selects 0 through 3 (default 3). Every input module, executable, runtime, explicit ThinLTO linker, and saved LLVM output must have a distinct file identity; symlinks and hard links cannot bypass the input overwrite check.

`--lto-linker PATH` enables optional cross-language ThinLTO and requires explicit `--runtime` and `--target TRIPLE`. Build that runtime with `RUSTFLAGS=-Clinker-plugin-lto` in a separate target directory, and select an LLVM linker that can read both the Rust and Clang bitcode. The tested combination is Clang 18 with the pinned Rust toolchain's bundled LLD. At optimization level 3, Relar raises LLVM's import and inlining budgets to 1,000 so runtime accessors can inline across the language boundary; this trades additional code size and link work for fewer runtime calls. Other optimization levels retain LLVM's normal budgets. `--target` also works without ThinLTO; it does not expand the supported runtime platforms.

```sh
RUSTFLAGS=-Clinker-plugin-lto cargo build -p relar-runtime --release --locked --target-dir target/lto
RELAR_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
RELAR_LTO_LINKER="$(rustc --print sysroot)/lib/rustlib/$RELAR_TARGET/bin/gcc-ld/ld.lld"
target/release/relar build examples/dependency_planner/__main__.py \
  --module-root examples --clang clang-18 --target "$RELAR_TARGET" \
  --runtime target/lto/release/librelar_runtime.a --lto-linker "$RELAR_LTO_LINKER" \
  -o /tmp/dependency-planner-lto
```

All five native test harnesses accept the same `--lto-linker` and `--target` options.

`--sanitize` marks generated functions for AddressSanitizer and links its runtime. This instruments generated code; it does not retroactively instrument a prebuilt Rust static library. Instrumenting Rust runtime code requires a separate sanitizer build of that library. The test suites accept `--sanitize --opt-levels 1` to exercise this configuration, including leak checks on normal exits.

The differential suite compares full stdout with CPython at `-O0` and `-O3`, covering 260 integer pairs, aliases, ownership across calls and replacement, evaluation order, short-circuiting, loops, and runtime errors. Separate rejection cases cover type violations, ignored errors, unsupported dynamic operations, and recursive calls. These are correctness checks, not performance measurements.

The imports suite compares multi-file programs with CPython at `-O0` and `-O3`, including package entry points, aliases, re-exports, relative imports, late function-global lookup, and mutation across module boundaries. It also checks invalid imports, unused invalid code, source diagnostics, and protection of imported files from output writes.

The classes suite checks nominal types, imported class aliases, field replacement, temporary receivers, returns, method argument evaluation, and normal-exit ownership at `-O0` and `-O3`. Rejection cases cover incomplete construction, escaping `self`, invalid writes, dynamic class features, recursive calls, and cyclic field types.

The containers suite covers nested lists, tuple positions and unpacking, list iteration and `enumerate`, optional ownership and local flow narrowing, contextual literals, shallow repetition, string concatenation, and whitespace splitting. Iteration cases exercise alias mutation, source and target rebinding, temporary roots, control flow, and arbitrary-precision counters. It rejects invalid alias writes, unsafe optional accesses, and class cycles hidden inside containers.

The I/O suite checks complete output, large and Unicode writes, normal and fatal exit flushing, LF and CRLF pipe input, terminal line visibility, and output failures. Interactive checks use protocol handshakes and readiness deadlines rather than performance expectations. All five suites also run generated-code AddressSanitizer checks at `-O1` in CI.
