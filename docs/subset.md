# Executable prototype contract

This is a deliberately restricted Python dialect for closed first-party programs. The current target is 64-bit Linux with LLVM/Clang 18. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

## Accepted source

| Area | Supported behavior |
| --- | --- |
| Modules | Closed first-party UTF-8 source graph; definitions and imports precede executable entry statements; optional module/function docstrings |
| Imports | Absolute, `from`, and package-relative imports; aliases and package re-exports; fixed module and definition bindings; no import cycles |
| Functions | Required parameter and return annotations; positional and keyword calls; positional-only and keyword-only parameters; scalar literal defaults; direct, fixed bindings; no recursive call cycles |
| Types | `int`, `float`, `bool`, `str`, `bytes`, `None`, sealed `typing.TextIO` and `typing.BinaryIO`, the sealed builtin `Exception` family, fixed nominal classes, concrete `list[T]`, `dict[K, V]`, and `set[T]`, fixed tuples such as `tuple[str, int]`, and `T \| None`; locals have one inferred or declared storage type |
| Classes | Declared fields, an explicit initializer, typed instance methods, alias-visible field mutation, and numeric field augmented assignment |
| Statements | Single-target and fixed-tuple unpacking assignment, annotated assignment, numeric augmented assignment, local string/bytes `+=` and bytes `*=`, dictionary entry deletion, `if`/`elif`/`else`, `while`, `for` over ranges, strings, bytes, or concrete containers, `break`, `continue`, `return`, `pass`, `assert condition[, string_message]`, `raise`, `try`/`except`/`else`/`finally`, and builtin-file `with` |
| Conditions | Explicit `bool`; `and`, `or`, `not`; short-circuit evaluation; conditional expressions `yes if condition else no`; `is None`/`is not None` with local flow narrowing |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`; `/` returns a correctly rounded float; unary `+`/`-`; all six value comparisons, including chains |
| Floats | Binary64 literals and exact storage; `+`, `-`, `*`, `/`, `//`, `%`; unary signs; all six comparisons; mixed integer arithmetic and exact mixed comparisons; explicit numeric/string conversions |
| Lists | One exact element type, including nested containers and classes; literals, shallow repetition and slicing, checked negative indexing, indexed assignment, iteration, `append(value)`, `pop()`, `len(...)`; alias-visible mutation |
| Dictionaries | Exact `int`, `str`, or `bytes` keys and one concrete value type; literals, contextual `dict()`, lookup/write/delete, numeric augmented writes, `in`/`not in`, `get`, `len`, and insertion-ordered key/items iteration |
| Sets | Exact `int`, `str`, or `bytes` elements; literals, contextual `set()`, `add`, `discard`, `in`/`not in`, `len`, and iteration with unspecified order |
| Comprehensions | Eager list, dictionary, and set comprehensions; nested `for` clauses and explicit boolean filters over the existing iteration sources; isolated target bindings |
| Tuples | Fixed length and a type per position; literals, bounded literal indexing, unpacking into names, and `len(...)`; immutable slots |
| Strings | Immutable UTF-8 text, literals, scalar f-strings, concatenation, equality/inequality, Unicode scalar length, indexing, slicing and iteration, no-argument whitespace `split()` and `splitlines()`, input and output |
| Bytes | Immutable bytes; literals, construction from an integer count or `list[int]`, concatenation/repetition, all value comparisons, indexing/slicing/iteration, `len`, Python byte repr, strict UTF-8 encode/decode |
| Files and arguments | Explicit UTF-8 text read/write and binary read/write/append/exclusive-create modes; `read`, `write`, `flush`, `close`, `closed`; `with` cleanup; one shared mutable `sys.argv` list |
| Builtins | `print(value)` for one scalar or exception; `input()` without a prompt; `int(int_or_float_or_bool_or_string)`; `float()` or `float(int_or_float_or_bool_or_string_or_bytes)`; `str()` or `str(int_or_float_or_bool_or_string_or_bytes_or_None_or_exception)`; `len(string_or_bytes_or_container)`; contextual empty `dict()` and `set()`; `range` with one to three integer arguments and `enumerate(string_or_bytes_or_container[, start])` at a loop or comprehension boundary |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. An empty list defaults to `list[int]` unless a typed storage, call, or return boundary supplies its element type. No annotation permits `Any`, a general union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Relar additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Relar's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

Mutable global values, first-class functions or methods, closures, decorators, reflection, dynamic attributes, generators, async, assignment expressions, slice assignment/deletion, starred or variable-length unpacking, variadic parameters, defaults other than scalar literals, loop `else`, mutable-list augmented assignment, and `is`/`is not` comparisons other than None checks are currently rejected. Conditions cannot rely on implicit truthiness. Unsupported operations are rejected even in dead branches and unused functions. A recursive call graph is rejected, including indirect recursion through methods, constructors, or comprehension bodies; a future recursion feature needs an explicit stack policy.

## Binary64 numbers

`float` is an exact storage type. It uses IEEE 754 binary64 in native arguments, returns, locals, fields, tuples, lists, and dictionary values. An annotation never silently changes a Python value: `value: float = 1` and passing an `int` to a `float` parameter are rejected; write `float(1)` explicitly. Mixed arithmetic itself performs Python's integer-to-float conversion and may raise `OverflowError` for an integer too large to convert. Augmented assignment must leave the target's exact type unchanged, so an integer slot cannot use `/=`.

Native arithmetic uses no fast-math flags. Signed zeros, infinities, NaNs, and ordinary rounding remain observable. Float division by either signed zero raises `ZeroDivisionError`. Floor division and modulo follow Python's corrected remainder convention, including `1.0 // 0.1 == 9.0` and the divisor's sign on a zero remainder. Overflow in ordinary float arithmetic can produce infinity. Comparisons with NaN are false except `!=`; mixed integer/float comparisons compare exact values without rounding the integer to binary64.

Integer `/` computes the ratio directly and rounds once, including at the subnormal boundary. Neither operand needs to fit in a float: two equal 400-digit integers divide to `1.0`. A result too large for binary64 raises `OverflowError`; underflow preserves the result's zero sign. `int(float)` truncates toward zero, raises `ValueError` for NaN, and raises `OverflowError` for infinity.

`float(str)` accepts signed decimal notation, optional separators between digits, surrounding whitespace, and case-insensitive `inf`, `infinity`, and `nan`. Decimal parsing is correctly rounded; overflow and underflow produce infinity and signed zero. Unicode decimal digits use the pinned ICU character database, whose Unicode version can be newer than a particular CPython build. `float(bytes)` accepts the same decimal grammar restricted to ASCII bytes; non-ASCII bytes raise `ValueError` without Unicode decoding. Malformed input raises `ValueError`. `print`, `str`, and scalar f-strings use shortest round-trip decimal formatting with Python's decimal tie-breaking, exponent signs, and `.0` for integral values in fixed notation. General format specifications remain unsupported.

`float | None` preserves every binary64 bit pattern by boxing only present optional payloads. None remains the nullable zero word; narrowing loads the double while the box is rooted. This costs an allocation when promoting an ordinary float to optional storage, without reserving a NaN payload or conflating absence with `0.0` or `-0.0`. Float dictionary keys, set elements, complex numbers, exponentiation, float-specific methods, and implicit truthiness remain outside the contract.

## Conditional expressions and sequence access

`yes if condition else no` evaluates its boolean condition once and then evaluates only the selected arm. Both arms are checked, including dead arms, and must have the same concrete type or fit one supported `T | None`. A typed assignment, argument, or return supplies context to both arms, including empty containers. Local optional narrowing applies separately inside each arm and does not leak past the expression. For example, `value if value is not None else 0` has type `int` when `value` is `int | None`.

String indexing uses an exact `int` and returns a one-code-point string. Negative indexes count from the end; out-of-range indexes raise `IndexError`. Strings remain valid UTF-8, so indexing, slicing and iteration operate on Unicode scalar values, including combining marks and supplementary characters, rather than bytes or grapheme clusters.

`sequence[start:stop:step]` accepts a concrete list, string, or bytes and returns that same type. Typed boundaries supply context through a slice to literal receivers, so `return [][:]` works in a function returning `list[str]`; an existing list cannot change its element type. Each component may be an `int`, `None`, or `int | None`; omission is equivalent to `None`. Booleans are not integer bounds. Bounds and steps may have arbitrary precision, negative steps reverse traversal, bounds are clipped to the sequence, and a zero step raises `ValueError`. In particular, `values[::-1]` and `values[:-1:-1]` have different meanings. List slices create a separate list with shallow copies of the selected element references, preserving alias-visible mutations of contained objects.

The receiver evaluates first, followed by start, stop and step in source order. All remain owned until the read completes; mutations performed by a bound expression affect the slice's current contents and length. A failed evaluation prevents later expressions and the read itself. Tuple slicing, slice objects as user values, string item assignment, and slice assignment/deletion remain unsupported.

String loops, comprehensions, and loop-head `enumerate` capture the original immutable string once and advance by code point in linear time. Each yielded character has its own owner; replacing the source binding or field cannot invalidate the iterator or an earlier character. Repeated arbitrary string indexing may scan UTF-8, so use iteration when visiting every character.

## First-party module graphs

`relar check` and `relar build` resolve imports under one module root. By default this is the entry file's directory, or the parent of its outermost package when enclosing directories contain `__init__.py`. `--module-root DIRECTORY` selects the root explicitly. An entry inside a package follows `python -m package.program` semantics: enclosing package initializers are loaded and relative imports are valid. A package `__main__.py` can be the entry; `__init__.py` itself cannot.

For example, `import package.operations` binds `package`, `import package.operations as operations` binds the child module, and `from package.operations import solve as run` binds the same function under `run`. Module-qualified calls and imported calls resolve to their defining module's namespace. Function bodies can call a name imported later in that module, provided all definitions and imports precede executable statements. Module and definition bindings cannot be reassigned or shadowed by parameters or locals.

Imported modules, including every package `__init__.py`, may contain only definitions, imports, docstrings, and an optional `if __name__ == "__main__":` guard without an `else`. Every loaded module is type-checked and lowered for validation, including unused functions and inactive guards. Only the entry module's executable statements run. The fixed `__name__` string is `"__main__"` in the entry and the qualified module name in imported code.

Imports are resolved statically from `.py` files and packages with `__init__.py`. Wildcards, dynamic or function-local imports, namespace packages, import cycles, external dependencies, and general standard-library imports or shadowing are rejected. The compiler recognizes sealed bindings for `typing.TextIO`, `typing.BinaryIO`, `sys.argv`, and `io.UnsupportedOperation`; these do not execute standard-library Python code. Their import aliases and first-party re-exports preserve identity. A package can re-export definitions from its children; reading an unfinished parent's definitions through a circular import is rejected. Symlinks cannot escape the module root or give one source file multiple module identities. The source-string library APIs accept sealed bindings but remain isolated from filesystem imports, which use the path-based APIs.

The I/O surface is intentionally small. `input()` reads a UTF-8 line and removes its trailing LF. A preceding CR remains part of the value, matching CPython's piped input on the Linux target. `int(str)` accepts signed ASCII decimal digits with optional separators between digits and surrounding whitespace. Other bases and Unicode decimal digits in input strings are outside this prototype's input format. Surrogate escapes in non-raw string literals are rejected: Ruff's analysis AST substitutes replacement characters for them, which is unsuitable for executable semantics. Strings support `+` and local `+=`; `text.split()` groups Python whitespace and omits empty fields, returning `list[str]`. `text.splitlines()` recognizes LF, CR, CRLF, vertical tab, form feed, U+001C through U+001E, U+0085, U+2028, and U+2029. It returns no elements for empty input and no extra element for a final line break; U+001F remains ordinary text. Separators, split limits, retained line endings, other string methods, general formatting, and arbitrary codecs are not implemented. CPython's configurable integer conversion digit limit is not reproduced.

## Function arguments and defaults

First-party functions, instance methods, and constructors accept positional and named arguments. Parameters before `/` are positional-only; parameters after a bare `*` are keyword-only. Other parameters accept either form. Imported aliases and re-exports retain the defining signature. Every call is bound during compilation, including checks for missing, duplicate, unexpected, or incorrectly typed arguments.

```python
def adjust(value: int, /, offset: int = 0, *, label: str = "stock") -> str:
    return f"{label}: {value + offset}"

print(adjust(8))
print(adjust(8, label="orders", offset=2))
```

Defaults must be `int`, `float`, `bool`, `str`, `bytes`, or `None` literals. A number may have one unary `+` or `-`; integers include arbitrary-precision hexadecimal, octal, and binary literals. Ordinary string/bytes-literal rules also apply to defaults. Each default must fit its parameter's exact type: `True` is not an `int` default, and `None` requires `None` or an optional annotation. Mutable containers, tuples, names, calls, arithmetic expressions, and f-strings cannot be defaults, even in an unused function. Literal defaults do not require mutable module state or a general function-object representation.

The receiver of an instance call evaluates first. Supplied arguments then evaluate once in source order, independently of parameter order; a failure prevents later arguments and the callee from running. The compiler applies each parameter's exact type context to its argument, including empty containers and optional values, and retains evaluated values across later mutations and failures. Named arguments lower to ordinary positional native calls.

Variadic parameters and `*`/`**` argument expansion remain unsupported. Builtins keep their existing argument conventions; the explicit `encoding="utf-8"` on `open` is their only accepted keyword. First-party keyword support does not add keywords to `print`, container methods, or file methods.

## Scalar f-strings

F-strings interpolate exact `int`, `float`, `bool`, `str`, `bytes`, `None`, and sealed builtin exception values. Ordinary, raw, triple-quoted, and adjacent literals are supported, including escaped braces, Unicode, and escaped NULs. Each field expression and its conversion complete before the next field evaluates. Nested f-strings may appear inside field expressions. Optional values require the same local narrowing as other scalar operations.

```python
def describe(name: str, quantity: int) -> str:
    return f"{name}: {quantity} units"
```

The default conversion and `!s` use the existing sealed scalar string conversion, selected directly by the compiler. The existing restriction against rebinding builtin names still applies. An absent or explicitly empty format specification, as in `f"{quantity:}"`, is supported. Nonempty or dynamic format specifications, `!r`, `!a`, and debug `=` fields are rejected, including `f"{quantity=!s}"`. General `__format__`, `__str__`, and `__repr__` protocols remain unsupported.

Raw prefixes apply only to their own literal text. Expressions inside a raw f-string still follow their ordinary string rules, and adjacent parts keep their individual prefixes. Surrogate escapes in non-raw literal text or field expressions are rejected consistently with ordinary strings; tabs and form feeds inside literal text remain data.

## Immutable bytes

`bytes` is an exact immutable type distinct from `str`. Ordinary, raw, escaped, and adjacent byte literals use Python's literal rules. Every byte from 0 through 255 is valid, including NUL and invalid UTF-8. Bytes can appear in classes, tuples, lists, dictionary keys/values, sets, and optionals. Literal bytes are also accepted as immutable parameter defaults.

`bytes()` returns empty bytes; `bytes(count)` creates that many zero bytes; `bytes(list_of_ints)` checks each integer is in 0 through 255; and `bytes(existing_bytes)` preserves its value. Counts must fit a native signed index before the nonnegative check: negative representable counts raise `ValueError`, unrepresentable counts raise `OverflowError`, and out-of-range list values raise `ValueError`. Booleans do not satisfy these exact integer inputs. Other iterable constructors and the string-plus-encoding constructor are outside this milestone.

Indexing uses exact integer indexes, including negative indexes, and returns an `int` in 0 through 255. Out-of-range indexes raise `IndexError`. Slicing follows the sequence rules above and returns bytes. Iteration, comprehensions, and loop-head `enumerate` yield integers in byte order while retaining the original immutable source independently of rebinding. Concatenation requires two byte values; repetition accepts bytes and an exact integer in either order. Repetition converts the count to a native signed index even for empty input and rejects an overflowing result. All six comparisons use unsigned-byte lexicographic order. `print`, `str`, and scalar f-strings use Python's `b'...'` representation, including quote selection and hexadecimal escaping.

`text.encode()` produces UTF-8 bytes; `data.decode()` performs strict UTF-8 decoding. They accept no arguments or keywords in this milestone. Decode errors raise `UnicodeDecodeError`, including Python's byte positions/ranges and invalid-start, invalid-continuation, or truncated-sequence message. Existing strings contain only Unicode scalar values, so encoding is always valid. Mutable `bytearray`, memory views, implicit text/byte conversions, additional codecs, error policies, and other byte methods are not supported.

## UTF-8 files and process arguments

`open(path, "r", encoding="utf-8")` and `open(path, "w", encoding="utf-8")` return a sealed `typing.TextIO` handle. The path is a string expression; mode and encoding must be the shown literals. Read mode opens an existing file; write mode creates or truncates a file. There is no locale-dependent default encoding. Handles can be passed, returned, stored in fields or containers, and aliased; every alias observes the same open/closed state.

`read()` consumes the remaining file and decodes strict UTF-8, translating CRLF and bare CR to LF. Further reads at EOF return the empty string. `write(text)` writes UTF-8 and returns the number of Unicode scalar characters, including embedded NULs. `flush()` makes buffered output visible to the operating system; it does not promise durable storage. `close()` flushes and closes, marks the shared handle closed even on a close failure, and succeeds on repeated calls. `.closed` is read-only. Entering, reading, writing, or flushing a closed handle raises `ValueError`. Reading a write-only handle or writing a read-only handle raises `io.UnsupportedOperation`, which matches both `OSError` and `ValueError`.

`open(path, "rb")`, `open(path, "wb")`, `open(path, "ab")`, and `open(path, "xb")` return a sealed `typing.BinaryIO` handle. Binary calls accept no encoding or other keyword arguments. Read mode opens an existing file, write mode creates/truncates, append mode creates if absent and appends, and exclusive-create mode raises `FileExistsError` if the path already exists. `read()` returns all remaining raw bytes without newline conversion; `write(bytes)` returns the number of bytes written. EOF returns `b""`. Binary handles support the same shared ownership, `flush`, `close`, `.closed`, and context cleanup as text handles. Their access mode survives closure: reading a write-only stream or writing a read-only stream raises `io.UnsupportedOperation` even after it closes; supported operations on a closed stream raise `ValueError`.

`with` accepts only these builtin handles, with optional name-only `as` targets and multiple managers. It evaluates and enters them left to right and closes entered handles in reverse order. Cleanup runs on fallthrough, errors, returns, and loop transfers. A hidden owner retains the entered file independently of any rebinding of its visible target. A later manager's failure closes earlier managers. A close failure replaces an earlier exception or return, while outer cleanup still runs. The target remains bound after normal exit, usually to the now-closed file.

Final reference release closes a file best-effort without callbacks or a new pending exception. Use explicit `close` or `with` to observe close errors. Text append modes, update modes, seeking, file iteration, partial reads, arbitrary codecs, newline options, generic context-manager protocols, and user finalizers are outside this contract.

Invalid UTF-8 raises `UnicodeDecodeError`, matching `UnicodeError` and `ValueError`. Embedded NULs in paths raise `ValueError`. Missing files, permission failures, directories opened as files, and non-directory path components use `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`; exclusive-create collisions use `FileExistsError`, also an `OSError`; other operating-system errors use `OSError`. Runtime-generated message text is not a CPython compatibility promise.

`sys.argv` is one lazily initialized mutable `list[str]`, shared across aliases and modules. Its first element identifies the invoked native executable, corresponding to the script identity under CPython. Element writes and list mutation are supported; rebinding the module attribute or imported name is not. Native arguments must be valid UTF-8. Invalid Unix argument bytes raise `UnicodeDecodeError` when the list is first accessed; CPython's surrogateescaped strings are outside this runtime's string domain. Programs that do not use `sys.argv` emit no argument initialization or shutdown calls.

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

Every class requires `__init__(...) -> None`, including classes with no fields. Its body contains only straight-line `self.field = expression` assignments, docstrings, and `pass`. Every declared field must be assigned exactly once. Assignment values may use constructor arguments and checked expressions, but cannot read or pass the receiver; a partially initialized object never escapes construction. Local assignments, conditionals, loops, explicit returns, and direct later calls to `__init__` are rejected.

Inheritance, metaclasses, class variables, field defaults, properties, static/class methods, dynamically added fields, and special methods other than `__init__` are unsupported. Field and method names beginning with `__` are reserved, except the initializer. Class-field ownership types must form an acyclic graph, following list elements, dictionary values, tuple positions, and optional payloads. For example, fields `children: list["Node"]`, `named: dict[str, "Node"]`, and `parent: "Node | None"` inside class `Node` are rejected even when unused or initialized empty. Methods may accept or return their own class because those signatures do not create stored references.

## Concrete containers and optional values

`list[T]` has one concrete element type, such as `list[str]`, `list[Counter]`, or `list[list[int]]`. Types are invariant: an existing `list[str]` cannot become `list[str | None]` through assignment or a call. Every append and indexed write checks the exact element type. Repetition copies references to the same elements, so `[row] * 3` shares `row` across all three positions. `list[int]` retains its specialized tagged-integer storage and runtime calls; other lists use owned runtime slots.

`dict[K, V]` requires exactly `int` or `str` keys and one concrete value type. Keys use numeric or exact Unicode string equality; booleans cannot stand in for integer keys. Nonempty literals infer these types from their first entry; `{}` and `dict()` require a concrete contextual type. Duplicate keys keep the first insertion position and the last value. Replacing a value keeps its position, while deletion and reinsertion move the key to the end. `mapping[key]`, assignment, `del mapping[key]`, integer augmented assignment, `len`, and single `in`/`not in` comparisons are supported. Missing required reads and deletes report `KeyError`.

`mapping.get(key)` returns `V | None`, flattened when `V` is already optional or `None`. A default must fit `V | None`; a guaranteed `V` default gives a `V` result. Defaults are evaluated eagerly even when the key exists. Container literals in defaults receive the dictionary's declared value type. Dictionaries are invariant, and value comparisons, expansion with `**`, copy/iterable constructors, `keys`, `values`, and other mapping methods are not implemented.

`set[T]` requires exactly `int` or `str` elements, using the same scalar equality as dictionary keys. Nonempty literals infer their element type and evaluate every element from left to right, including duplicates. Empty `set()` requires a concrete contextual type. `add(value)` inserts an element, `discard(value)` removes it when present, and both return `None`. Adding a duplicate or discarding a missing element is a no-op. `len`, single `in`/`not in` comparisons, direct iteration, and loop-head `enumerate` are supported. Set iteration order is unspecified and programs must not depend on dictionary-like insertion order. Iteration checks size changes and owns each yielded element before the body can remove it. Copy/iterable construction, set algebra, comparisons, `remove`, `pop`, and `clear` are not implemented.

Tuple types name each position, for example `tuple[str, int, list[Counter]]`; `tuple[()]` is empty. A tuple's slots cannot be reassigned, but a referenced list or instance can still be mutated. Indexes must be in-range integer literals, optionally negative. Variable indexes, computed indexes, variadic tuple annotations, tuple concatenation, and tuple repetition are rejected. List and tuple value comparisons are unsupported.

Fixed tuples can be unpacked into names in assignments and loop targets, including nested tuple or list target syntax: `name, (count, values) = record`. Every source value is evaluated before any target is rebound, so swaps and repeated target names retain Python's assignment order. Targets cannot contain stars, attributes, or subscripts, and a list value cannot be unpacked because its length is not part of its type.

`for value in values` evaluates a concrete list once and retains that original list even if the source name or field is later replaced. It reads the current length and acquires the next element at each step. Appending, removing, or replacing elements through an alias therefore has Python's index-based list-iteration behavior. The current item remains owned if the body removes it from the list. `enumerate(values, start)` captures the list before evaluating its optional integer start and yields fixed `(int, T)` tuples; its counter supports arbitrary precision and is independent of visible target rebinding. Both forms support `break`, `continue`, nested loops, and early returns. A target first introduced by a loop remains potentially uninitialized afterward because the loop may be empty. Iterator values, tuple iteration, and user-defined iteration protocols are not supported.

`for key in mapping` and `for key, value in mapping.items()` visit insertion order. `enumerate` accepts either form and creates the iterator after evaluating its start argument. Each yielded key/value has its own owner before the body runs. Replacing an existing value is allowed and later iterations see the replacement. A changed size at the next step reports `RuntimeError`, including after the last yielded item. Same-size structural mutation does not provide a portable traversal guarantee; it may report changed keys or skip entries. `items()` is supported only at a loop boundary, including inside loop-head `enumerate`; views and iterators cannot be stored as user values.

Literal elements receive types from an annotated variable, an existing storage location, a function or method argument, or a return annotation. This also applies inside nested literals and list repetition: `rows: list[list[str]] = [[], []]` creates two independent string lists. It does not convert an existing container to a different element type.

`T | None` permits a value of exactly `T` or `None`. Storage, call, and return boundaries explicitly wrap a present value; a bare return or falling off an optional-returning function produces `None`. No other union is supported. Use `value is None` or `value is not None` before accessing an unknown optional payload. Direct checks of local names narrow the relevant branches, short-circuit operands, assertions, and guarded loop bodies; early exits preserve facts on the remaining path. Rebinding updates those facts, and possible stores in a loop invalidate facts that later iterations could contradict.

Narrowing applies to local bindings, not mutable fields or indexed elements. Copy a field or element into a local before checking it; the local owns its value even if an alias later replaces the original slot. Compare an optional payload only after narrowing it. Neither an optional nor a container supplies implicit truthiness.

## Eager comprehensions

List, dictionary, and set comprehensions support nested `for` clauses and zero or more explicit boolean filters per clause. Sources are the existing concrete lists, strings, dictionary keys or `.items()`, sets, `range`, and loop-head `enumerate` forms. Targets use the same name-only fixed-tuple unpacking as ordinary loops.

```python
def positives(rows: list[list[int]]) -> list[int]:
    return [value for row in rows for value in row if value > 0]


names: list[str] = ["alpha", "beta", "alpha"]
unique = {name for name in names}
last_position = {name: index for index, name in enumerate(names)}
```

The leftmost iterable uses the enclosing scope and is evaluated once. All clause targets belong to the comprehension's separate scope, including names first assigned by later clauses; they cannot fall back to an outer binding before assignment. Targets may shadow an enclosing local of another type and never replace that outer binding. Nested comprehensions have their own scope. Static module/definition bindings and reserved builtins retain their existing shadowing restrictions.

Clauses nest from left to right. Each inner iterable is evaluated when its enclosing iteration reaches it; filters short-circuit in source order, and the element expression runs only after all filters pass. Dictionary keys are evaluated and owned before their values. Duplicate dictionary keys retain their first insertion position and last value. Set traversal remains unspecified.

Results have exact element/key/value types, inferred from the checked body or supplied by a typed storage, argument, or return boundary. Context also reaches nested literals and comprehensions. Filters can narrow optional local targets before constructing an element. These rules do not convert an existing container to a different element type or permit implicit truthiness.

Capturing an outer local preserves its value and shared object identity; field and indexed reads still occur where written inside the body. Mutations through aliases retain the existing list/dictionary/set iteration behavior. A failure propagates through normal handlers and finally blocks, releasing the partial result and temporary owners without rolling back earlier user-visible mutations. Eager comprehensions do not expose an iterator, closure, callback, or partially built result. Generators, async clauses, assignment expressions, and new iteration protocols remain unsupported.

## Ownership and errors

Assignment and return expressions produce owned references when their type needs one. Operator and call operands may borrow a direct local: expressions cannot rebind a caller's slot in this closed subset. Parameter bindings that are never reassigned borrow their caller's root; other local slots own their values and release them on exit. A return acquires or transfers ownership even when returning a borrowed parameter. List-element reads generally remain owned across later evaluation; field and tuple reads, optional unwrapping, pops, and computed results are also owned, so mutation cannot invalidate an intermediate value. This also keeps a method receiver alive if later arguments replace its original field. An assignment publishes the replacement before dropping the old value. Ordinary subscript and field assignment evaluate the RHS first; augmented assignment instead captures the target and old value before evaluating its RHS.

Integer representation is canonical: values from `-2**62` through `2**62 - 1` always use tagged machine words; values outside that range use `num-bigint`. Checked arithmetic promotes and demotes as needed. Foreign adapters must preserve this invariant before passing integers to generated code.

Conservative interval facts track immutable integer values in local slots across reassignment and control flow. Heap reads and user-call results start unknown. Direct arithmetic requires proofs that both operands and the result fit the small representation; only proved representable arithmetic and tagging shifts receive LLVM's `nsw` overflow flag. Unknown bounds, analysis overflow, or a possibly zero divisor retain checked lowering. Division by zero remains a language error at every optimization level. `range` arguments are captured once, and a specialized hidden cursor must also fit after its final increment; assigning to the visible target does not change its progression.

Old-value and exit ownership checks can be omitted for an integer local only when every possible write or move into that slot is proved small. Parameters and unknown replacements retain those checks.

A proved nonnegative small index may use a decoded native-index ABI, but each read or write still checks the container's current bounds. Calls, alias mutation, and resizing cannot invalidate a cached length because no such length proof is used. Negative or arbitrary-precision indices retain their normal normalization and error behavior. Element reads acquire ownership before later evaluation can remove them. The narrow exception is a direct integer-list read in a two-operand integer comparison whose complete operand trees are proved nonmutating; the local keeps the list rooted until the comparison consumes the borrowed handle. Chained comparisons and potentially mutating evaluation retain owned reads. A same-list copy with simple, proved indices can also combine its get and set, checking and acquiring the source before checking and replacing the destination.

Heap objects are nonmoving reference-counted allocations. Classes and tuples use concrete fixed layouts, and homogeneous containers store their representation metadata once per container rather than in each element. Optionals occupy one nullable word: zero means absent, present integers and pointers keep their payload word, and optional booleans encode False/True as one/two. Present optional floats own a separate immutable binary64 allocation so every float bit pattern remains available; ordinary floats remain unboxed. These representations do not change the supported Python values or shared mutation behavior.

Writes acquire the replacement before releasing the displaced value, and reads acquire ownership before the receiver can be released, apart from the rooted comparison borrow above. Fixed records start with zeroed fields and use a static destruction descriptor, so failed construction safely releases every initialized owner. Generated field accesses are direct; their receiver remains rooted and the checker proves complete initialization before a read. Container storage uses scoped `UnsafeCell` closures on one execution thread: mutable access is exclusive, operations cannot reenter through aliases, and no storage reference or resizable-buffer address may escape. Independently owned values and explicitly constrained borrowed integer handles may escape; displaced owners are dropped after access ends.

Dictionaries use a randomized hash index and insertion-ordered entries. Integer, string, and byte keys have specialized storage; sets own keys without dummy value slots or an ordering guarantee. Internal iterators root their container and prevent entry compaction until released, but keep no storage reference across a loop body. The acyclic heap type graph prevents reference cycles through classes and containers. There are no finalizers or user callbacks under storage access. Threaded or reentrant extension boundaries would need to preserve these rules. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Language errors propagate through explicit checked call results and can be caught. Each fallible runtime call records an owned pending exception and reports failure through either its result or a pending-state query; generated code checks before decoding or acquiring a result. Integer-list reads reserve raw zero for failure, while writes return a success byte. Other calls retain the pending-state check. Proven fixed-field operations and functions whose complete bodies cannot raise omit redundant queries; their argument and receiver evaluations retain their own checks. Partial expression values, constructor fields, unpacking temporaries, and loop iterator roots are released when their scopes exit. Function propagation also releases owned locals. No Rust panic or native stack unwinding implements language exceptions. Uncaught errors print an exception category and message and return process status 1 after cleanup. Actual allocation failures and invalid internal ABI operations may still terminate the process. The compiler and runtime archive must be rebuilt together when this private ABI changes.

Exceptions contain only an immutable category and message. They have no traceback, frame, cause, or context references, so they introduce no cycles into the reference-counted heap. Full traceback objects, exception chaining, `raise ... from ...`, custom exception subclasses, exception groups, `except*`, and user-defined context managers remain unsupported.

## Exceptions and cleanup

The constructible builtin categories are `Exception`, `ArithmeticError`, `ZeroDivisionError`, `OverflowError`, `LookupError`, `IndexError`, `KeyError`, `ValueError`, `AssertionError`, `RuntimeError`, `EOFError`, `OSError`, `TypeError`, `AttributeError`, `MemoryError`, `SystemError`, `UnicodeError`, `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`, plus the imported `io.UnsupportedOperation`. A constructor takes zero arguments or one string message. `raise Category` constructs the zero-argument form; `raise value` requires an exception instance. The annotation `Exception` admits this sealed family and can be used in parameters, returns, fields, containers, and optional values. Specific subclass annotations are currently rejected rather than erasing their narrower contracts. `BaseException` and `UnicodeDecodeError` are supported as handler categories, but not as constructors or storage annotations.

Handlers select the first matching fixed builtin category or nonempty tuple of categories, using the builtin hierarchy. A bare handler catches any supported exception. `except Category as name` binds an owned exception and deletes that name on every handler exit, even if it previously existed or was rebound inside the handler. Save it under another name to retain it. Bare `raise` uses the dynamically active exception, including from a called function or a `finally` block unwinding an exception; without an active exception it raises `RuntimeError`.

`else` runs only after normal fallthrough from the protected body; its errors are handled outside that try statement. `finally` runs on normal fallthrough, error propagation, `return`, `break`, and `continue`. A new transfer from finally overrides the pending transfer and releases its owned return value or exception. Loop transfers inside finally use the block's lexical loop. Nested handlers restore the previously active exception.

Handlers cannot assume a fallible assignment completed. Definite assignment starts from the protected body's incoming state, excluding names that a nested handler could delete. Optional facts for potentially reassigned names are discarded. Finally is checked against a similarly conservative entry covering all exit reasons; initialize values before try when finally needs them. Integer interval facts in handlers and finally start unknown, and facts after try are conservative. These restrictions can reject safe programs requiring more precise exceptional-flow analysis.

`str(exception)` and `print(exception)` expose the message, including Python's special quoting for a one-string `KeyError`. Missing dictionary keys report the actual integer key or quoted string key. KeyError quoting follows Ruff's repr rules and pinned ICU Unicode data; newly assigned characters can print literally where a CPython build with older Unicode data emits an escape (for example U+1FAEA versus CPython 3.14). Other runtime-generated diagnostic messages are not a CPython text-compatibility promise; category matching and explicitly constructed messages are stable within this quoting rule. `assert` is always enabled independently of optimization, evaluates its optional string message only when false, and raises an empty-message `AssertionError` when no message is provided.

Standard output uses bounded buffering for pipes and files, and flushes each printed line on a terminal. Pending output is also flushed before `input()` waits, before a reported runtime error exits, and before the generated process entry returns normally. If a write or flush returns an error, Relar reports `OSError`; fatal-error flushing is best effort and preserves the original exception category. Generated code explicitly flushes at process exit because its LLVM entry point does not run Rust's standard `main` lifecycle.

Before creating file writers or initializing standard output, the runtime ignores `SIGPIPE` once so disconnected pipes produce catchable I/O errors. The sealed runtime has no user signal handlers or signal API.

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
python3 tests/comprehensions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/fstrings.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/calls.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/expressions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/exceptions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/files.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
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

All native test harnesses accept the same `--lto-linker` and `--target` options.

`--sanitize` marks generated functions for AddressSanitizer and links its runtime. This instruments generated code; it does not retroactively instrument a prebuilt Rust static library. Instrumenting Rust runtime code requires a separate sanitizer build of that library. The test suites accept `--sanitize --opt-levels 1` to exercise this configuration, including leak checks on normal exits.

The differential suite compares full stdout with CPython at `-O0` and `-O3`, covering 260 integer pairs, aliases, ownership across calls and replacement, evaluation order, short-circuiting, loops, and runtime errors. Separate rejection cases cover type violations, ignored errors, unsupported dynamic operations, and recursive calls. These are correctness checks, not performance measurements.

The imports suite compares multi-file programs with CPython at `-O0` and `-O3`, including package entry points, aliases, re-exports, relative imports, late function-global lookup, and mutation across module boundaries. It also checks invalid imports, unused invalid code, source diagnostics, and protection of imported files from output writes.

The classes suite checks nominal types, imported class aliases, field replacement, temporary receivers, returns, method argument evaluation, and normal-exit ownership at `-O0` and `-O3`. Rejection cases cover incomplete construction, escaping `self`, invalid writes, dynamic class features, recursive calls, and cyclic field types.

The containers suite covers nested lists, tuple positions and unpacking, list iteration and `enumerate`, optional ownership and local flow narrowing, contextual literals, shallow repetition, string concatenation, and whitespace splitting. Iteration cases exercise alias mutation, source and target rebinding, temporary roots, control flow, and arbitrary-precision counters. It rejects invalid alias writes, unsafe optional accesses, and class cycles hidden inside containers.

The comprehensions suite checks isolated and nested scopes, contextual result types, ordered filters and dictionary key/value evaluation, optional narrowing, alias-visible mutation, arbitrary-precision ranges, and cleanup after partial construction. It compares complete filesystem effects and rejects unsupported targets, escaped names, implicit truthiness, dynamic operations, and recursion hidden inside helpers.

The f-string suite compares literal decoding, raw and adjacent parts, scalar conversions, nested fields, optional narrowing, and comprehension captures. Side-effecting fields, later failures, mutable owners, and file writes check evaluation order and cleanup. Rejection cases cover unsupported formatting/conversions, debug fields, types, surrogate escapes, and builtin rebinding.

The calls suite compares positional/keyword binding, keyword-only parameters, scalar defaults, imported aliases, contextual container types, and optional values. Reordered arguments, temporary receivers, aliased owners, and later exceptions check evaluation order and cleanup. Rejection cases protect exact types, parameter kinds, required/duplicate names, literal defaults, and the positional builtin surface.

The expressions suite compares lazy conditional arms, contextual result types, optional narrowing, Unicode string indexing and iteration, and list/string slices. It checks 1,701 slices apiece for strings and integer lists, including omitted and arbitrary-precision bounds. Aliased shallow copies, operand mutation, later failures, and recovery check ownership and evaluation order. Rejection cases protect exact types, fixed tuple bounds, and read-only slicing.

The floats suite compares arithmetic and mixed-number matrices, shortest formatting, strict decimal conversion, exact comparisons, and large rational rounding against CPython. Boundary cases include signed zeros, NaNs/infinities, the subnormal and overflow thresholds, and integers too large to convert individually. Concrete classes, tuples, lists, dictionaries, optional values, defaults, and exception cleanup exercise native storage; rejection cases forbid implicit annotation-driven conversions.

The exceptions suite compares nested propagation, handler ordering, builtin hierarchy, stored exception values, dynamically scoped re-raising, lazy assertion messages, and all finally exit reasons. Repeated failures during argument evaluation and partial container/class construction exercise ownership cleanup. Rejection cases protect definite assignment, optional facts, and the sealed exception contract.

The I/O suite checks complete output, large and Unicode writes, normal and fatal exit flushing, LF and CRLF pipe input, terminal line visibility, and output failures. Interactive checks use protocol handshakes and readiness deadlines rather than performance expectations.

The files suite compares complete filesystem effects, strict decoding, newline handling, write counts, alias-visible closure, all `with` exit reasons, failed cleanup, argument aliases, and descriptor recovery under a reduced open-file limit. Rejection cases protect the sealed import, file, context-manager, and argument surfaces. The existing suites also run generated-code and runtime AddressSanitizer checks at `-O1` in CI.

The bytes suite compares full byte-range repr/indexing, strict UTF-8 decoding errors, constructors, exact typed storage and keys, 896 empty/nonempty slices, iterator ownership, and failures during partial construction at `-O0` and `-O3`. Binary fixtures compare complete file contents, append/exclusive modes, imported handle aliases, closed/wrong-direction errors, and every context-manager exit path. It accepts the same target, ThinLTO, and sanitizer options as the other native suites:

```sh
python3 tests/bytes.py --compiler target/release/relar --runtime target/release/librelar_runtime.a --clang clang-18
```
