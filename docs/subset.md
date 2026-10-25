# Executable prototype contract

This is a deliberately restricted Python dialect for closed, statically typed source graphs, including selected unchanged pure-Python libraries. Imre targets 64-bit Linux with Clang 18 and Apple Silicon macOS with Apple Clang. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

See the [standard library support table](stdlib.md) for module examples and native CPython test counts.

## Accepted source

Module functions with annotated results can use `functools.cache` or
`functools.lru_cache` with a literal capacity. The native cache accepts fixed
positional arguments with immutable key types. Bounded caches evict the least
recently used key; zero disables storage, and `None` keeps all results.
`cache_clear()` resets entries and counters. `cache_info()` returns a fresh
immutable `CacheInfo` tuple with named fields `hits`, `misses`, `maxsize`,
and `currsize`. An earlier snapshot keeps its values after another lookup or
clear. `cache_parameters()` returns a fresh
dictionary containing `maxsize` and `typed`. Applying a second cache to a
cached callable copies its public parameters; the outer wrapper still uses
its own capacity for eviction and for `cache_info()`.

`functools.partial` retains positional arguments for a synchronous callable with
a fixed signature. Calls fill positional `functools.Placeholder` slots first.
The partial can be returned, called repeatedly, or passed as a callback. Bound
methods retain their receiver; captured mutable arguments keep their identity.
Passing a placeholder last or as a keyword raises `TypeError`. Binding keyword
defaults and inspecting a partial's attributes are not yet supported.

| Area | Supported behavior |
| --- | --- |
| Modules | Closed UTF-8 source graph; imports can follow literal exports in imported modules; entry definitions precede executable statements; imported initializers run once in dependency/source order; optional docstrings |
| Imports | Absolute, `from`, and package-relative imports; aliases and package re-exports; fixed module, definition, and exported value bindings; shared mutable values; no import cycles |
| Copying | Shallow copies of mutable containers share their children. Plain fixed class instances copy their stored fields without rerunning the initializer; a class may override this through an inherited or explicit `__copy__(self)` method. Deep copy preserves aliases in closed builtin container graphs with immutable keys, using the implicit memo or literal `None`. |
| Functions | Exact signatures declared or inferred from concrete calls, scalar defaults, and checked bodies; direct positional and keyword calls; positional-only, keyword-only, and read-only variadic positional parameters; exact callable values, bound methods, and noncapturing lambdas; no recursive call cycles |
| Types | `int`, `float`, `bool`, `str`, `bytes`, `None`, sealed `pathlib.Path` and `subprocess.CompletedProcess[str]`, `typing.TextIO` and `typing.BinaryIO`, the sealed builtin `Exception` family, fixed nominal classes, string-valued `enum.StrEnum` and `typing.Literal`, concrete `list[T]`, `dict[K, V]`, and `set[T]`, fixed tuples such as `tuple[str, int]`, exact `typing.Callable[[T1, T2], R]` signatures, `str \| bytes`, and `T \| None`; locals have one inferred or declared storage type |
| Classes | Fixed fields, explicit or dataclass-generated initialization, typed instance/class/static methods, properties with optional setters, method-local `typing.Self`, alias-visible mutation, and frozen dataclasses |
| Statements | Single-target and chained assignment; nested and starred unpacking of fixed tuples, lists, strings, and bytes, annotated assignment, numeric augmented assignment, local string/bytes `+=` and bytes `*=`, dictionary entry deletion, `if`/`elif`/`else`, structural `match`/`case`, `while`, `for` over ranges, strings, bytes, concrete containers, or supported iterator pipelines, loop `else`, `break`, `continue`, `return`, `pass`, `assert condition[, string_message]`, `raise`, `try`/`except`/`else`/`finally`, and builtin file/workspace `with` |
| Conditions | Truth tests for every supported concrete type; operand-returning `and`/`or`, boolean `not`, and short-circuit evaluation; conditional expressions `yes if condition else no`; assignment expressions `(name := value)`; local optional narrowing |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`, `&`, `\|`, `^`, `<<`, `>>`, and `divmod`; `/` returns a correctly rounded float; unary `+`/`-`/`~` and `abs`; all six value comparisons, including chains |
| Floats | Binary64 literals and exact storage; `+`, `-`, `*`, `/`, `//`, `%`; unary signs and `abs`; all six comparisons; mixed integer arithmetic and exact mixed comparisons; explicit numeric/string conversions |
| Lists | One exact element type, including nested containers and classes; literals and starred displays, `list()` and `list(iterable)`, stable `sorted` with scalar or fixed-tuple keys, shallow concatenation, repetition, and slicing, checked negative indexing, indexed assignment, iteration, `append(value)`, `extend(source)`, `pop()`, `len(...)`; alias-visible mutation |
| Dictionaries | Exact `int`, `str`, string enum, `bytes`, `Path`, or immutable fixed-tuple keys and one concrete value type; literals and `**` displays, contextual `dict()`, lookup/write/delete, numeric augmented writes, `in`/`not in`, `get`, `setdefault(key[, value])`, `len`, and insertion-ordered key/items iteration |
| Sets | Exact `int`, `str`, string enum, `bytes`, `Path`, or immutable fixed-tuple elements; literals and `*` displays, contextual `set()`, `add`, `discard`, `in`/`not in`, `len`, and iteration with unspecified order |
| Comprehensions | Eager list, dictionary, and set comprehensions; immediately consumed generator expressions; nested `for` clauses and truth-tested filters; isolated target bindings |
| Tuples | Fixed tuples have one type per position; starred displays in function bodies can produce runtime-sized tuples. Literals, checked indexing, nested/starred unpacking into names, fields, and subscripts, and `len(...)`; immutable slots; value equality for immutable field shapes and lexicographic ordering for ordered scalar shapes |
| Strings | Immutable UTF-8 text, literals, scalar, record, and list f-strings and `!r`, named `str.format` fields, signed decimal integer and bounded fixed/percent formats, `textwrap.dedent`, concatenation, equality/inequality, Unicode scalar length, indexing, slicing and iteration, bounded `split`/`rsplit`, `splitlines()`, stripping, prefix/suffix checks, `find`, `partition`, `isspace`, `join(iterable)`, `replace(old, new)`, input and output |
| Regular expressions | Fixed text patterns in `re.compile` and `re.search`, named and numbered groups, character offsets, and `re.VERBOSE` / `re.UNICODE` |
| Bytes | Immutable bytes; literals, construction from an integer count or `list[int]`, concatenation/repetition, all value comparisons, indexing/slicing/iteration, `len`, Python byte repr, strict UTF-8 encode/decode |
| Paths | Native POSIX `pathlib.Path`; lexical joining and components, equality, text I/O, filesystem predicates, directory creation/removal, unlink/replace, directory iterators and recursive globbing |
| Workspaces | Stored or direct `TemporaryDirectory` and `nullcontext` scopes with typed enter values and recursive cleanup |
| Files and arguments | Explicit UTF-8 text and binary file modes, buffered line iteration, binary `read(n)`, read-only `gzip.open`, shared cursors, `with` cleanup, and one shared mutable `sys.argv` list |
| Processes | Synchronous `subprocess.run` with string arguments, captured UTF-8 text, working directories, replacement environments, checked status, and `os.environ.copy()` snapshots |
| Builtins | `print(value)` for one scalar, path, exception, or record; `repr(value)` for supported data values; `input()` without a prompt; `bool()` or `bool(value)`; `int(int_or_float_or_bool_or_string)`; `float()` or `float(int_or_float_or_bool_or_string_or_bytes)`; `str()` or `str(int_or_float_or_bool_or_string_or_bytes_or_None_or_path_or_exception_or_record)`; `abs(int_or_float_or_bool)`; `len(string_or_bytes_or_container)`; `list([iterable])`; `sorted(iterable, key=callback_or_None, reverse=bool)`; integer `sum(iterable[, start])`; `min(integer_generator)` and `max(integer_generator)`; `any(iterable)` and `all(iterable)`; immediate `map(callback, iterable)`, `filter(predicate_or_None, iterable)`, `zip(iterable, ..., strict=condition)`, `reversed(sequence)`, and `enumerate(iterable[, start])`; contextual empty `dict()` and `set()`; `range` with one to three integer arguments at an iteration boundary |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. A fresh local initialized with `rows = []` can take its element type from the first direct `rows.append(value)` statement later in the suite, including inside a loop or conditional. This inference supports scalar `int`, `bool`, `float`, `str`, and `bytes` operands. An empty list otherwise defaults to `list[int]` unless a typed storage, call, or return boundary supplies its element type. No annotation permits `Any`, an unrestricted union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Imre additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Imre's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

The compiler bounds syntax complexity before parsing each module. Deeply nested blocks, delimiters, or long chains of operators can exceed the limit. Splitting a long expression into intermediate assignments reduces its depth.

Global rebinding, capturing closures, arbitrary decorators, reflection, dynamic attributes, general generator functions, escaping iterator values, async generators, slice assignment/deletion, open-ended call argument expansion, untyped variadic parameters, function defaults other than scalar literals, mutable-list augmented assignment, and arbitrary object identity comparisons are currently rejected. By default the compiler checks every function and branch. A closed entry profile can select particular branches of reachable functions. A recursive call graph is rejected, including indirect recursion through callable values, methods, constructors, or comprehension bodies; a future recursion feature needs an explicit stack policy.

## Inferred function signatures and iterable consumers

Parameter and return annotations are optional when the closed source graph supplies enough concrete evidence. A missing parameter type comes from its scalar literal default or a known direct call. Once all parameters are concrete, the checker infers the return type from the body, including supported nullable returns and implicit `None` on a falling-through path. Evidence propagates through imported functions, direct methods, constructors, and comprehension bodies until every signature is complete.

```python
def checksum(text):
    digits = list(map(int, text))
    return sum(digits) % 10

print(checksum("12345"))
```

This infers `checksum(str) -> int`. Every function has one exact signature, and all bodies are checked again against the completed signatures before code generation. An annotation remains an enforced contract. Incompatible callers, an uncalled parameter without a default or annotation, recursive call cycles, and unsupported return unions are rejected. There is no dynamic fallback, implicit numeric conversion, arbitrary `Any`, or polymorphic specialization. A `None` default supplies the exact `None` type; annotate a parameter explicitly when it accepts a nullable payload. Inference does not solve arbitrary constraints backward from a function body.

`list()` creates an empty list with the contextual element type, defaulting to `list[int]`. `list(source)` makes a shallow copy of a concrete list, string, bytes, dictionary keys/items, set, range, enumeration, or fixed tuple. All copied elements must fit one supported destination type; an existing source container retains its own layout and alias behavior.

`left + right` evaluates both lists before copying their elements into a fresh list. Both inputs have the same element type. If an element is a nested list or a record, the result shares that object with the input lists.

`in` and `not in` support integer and string lists. An integer needle may be optional: a subprocess return code of `None` is absent from `[0, 1]`. Integer membership uses value equality, including for integers too large to fit in a machine word.

`sorted(source, key=callback, reverse=condition)` returns a fresh list. Both keywords are optional. `key=None` compares the elements themselves, and `reverse` defaults to false and requires an exact `bool`. Keys may be exact integers, booleans, strings, string enums, bytes, or fixed tuples recursively containing those scalar types. Tuple fields whose exact type is `None` are also allowed; nullable fields, floats, mutable containers, and records are not. Every key has the same concrete shape, and tuples compare lexicographically. Input and keyword expressions evaluate in source order, then the source is fully materialized before any key callback runs. Each key is evaluated once in input order. Sorting is stable in both directions, so equal keys retain their input order even with `reverse=True`. A callback may mutate the original source or its elements and may raise. The copy already contains all input elements when the first callback runs.

`values.sort(key=callback, reverse=condition)` accepts the same keys and updates the list in place. It returns `None`. Aliases observe the new order. During key evaluation the list appears empty until a callback adds elements. If a key raises, the list regains its original membership and order and the exception propagates. If keys succeed after a callback appends to the list, the sorted original elements are restored and the sort raises `ValueError`.

`functools.reduce(function, iterable[, initial])` folds a native iterable from left to right. The optional seed can also be passed as `initial=`. Without it, the first item becomes the accumulator and an empty input raises `TypeError`. The binary callback can be an exact function, bound method, or noncapturing lambda. Its return value must fit the accumulator's layout on every step.

`sum(source[, start])`, including `start=`, consumes exact integers and defaults to zero. Its source can be any supported concrete iterable whose yielded values are integers, including fixed tuples such as `divmod(...)`. An immediate generator may also yield exact booleans. For example, `sum(file.passes for file in files)` counts the true values. The generator acquires its outer iterator before `start` runs, then evaluates each projection once. The start and the result are exact, arbitrary-precision integers, including for an empty generator.

`math.prod(source, *, start=1)` consumes an integer iterable in order and multiplies using arbitrary-precision integers. An empty iterable returns the integer start unchanged. A zero factor does not stop iteration or mask an exception from a later element.

`map(str, source)` supports scalar values, paths, and records with a checked string conversion. Conversion happens as each item is pulled.

An immediate `map(int, source)` converts each item using the ordinary checked `int` conversion. Inputs may yield exact integers, floats, booleans, or strings. Conversion proceeds lazily as the enclosing consumer iterates; input expressions, iterator creation, the optional sum start, and conversion failures retain Python evaluation order. Mutable sources remain live and keep the existing dictionary/set mutation checks. User callbacks follow the exact callable contract below. A map cannot escape into storage, and user-defined iteration protocols remain unsupported.

`itertools.repeat(object[, times])` retains the original object. Aliases, `iter(it)`,
`next(it[, default])`, and ordinary loop and list consumers share its position.
Omitting `times` repeats forever; a nonpositive integer exhausts immediately.
`operator.length_hint(it[, default])` returns the remaining finite count or the
default for an unbounded repeat. A finite repeat's repr shows the current count.

`itertools.islice(source, stop)` and `islice(source, start, stop[, step])`
consume only the underlying items needed to reach each result. Exhausting a
finite slice advances a sufficiently long source to `max(start, stop)`. The
cursor releases its source on exhaustion or a pull error. `cycle(source)`
retains each item from its first pass and then replays those same objects.
Sources include supported cursors, ranges, lists, strings, bytes, dictionary
keys, sets and homogeneous fixed tuples. A list stays live during iteration.
`count(start=0, step=1)` supports integers of any size and floating-point starts
with integer or floating-point steps. A floating-point step with an integer
start requires a mixed numeric element type and is currently unsupported.

`tee(source[, n])` forks one source cursor. Each fork reads each buffered
item at its own pace. Forking an existing fork copies its current position and
shares the buffer. Deleting a bound cursor local drops that reference. The
current fixed-tuple return layout requires a literal fork count from 0 to 256.

`itertools.chain` accepts a fixed number of positional iterables with one exact
element type. A loop, comprehension, or immediate consumer can use the chain.
It evaluates source expressions left to right, then acquires each iterator on
reaching its source. A second `Path.glob` sees directory changes made while
consuming the first glob.

`list(difflib.ndiff(before, after))` compares two string lists, including supported
read-only `Sequence[str]` parameters. Line alignment, intraline Unicode hints,
and popular-line handling follow CPython 3.12 with its default junk filters.
The compiler can also pass the result to a parameter consumed immediately by `list`.
The list snapshots both inputs. Changing an input or a shallow copy afterward
preserves ordinary list semantics. A program that selects added or removed rows
and sorts by the text after the two-character tag can skip intraline comparisons.
Reading the raw rows yields the original ordering and `? ` guides.

`unidiff.PatchSet(text)` reads unified text patches, including Git file metadata.
Iterating a patch set yields files; iterating a file yields hunks. The supported
projections are `patch_set.modified_files` (a fresh list), `file.path`,
`hunk.source_start`, and `str(hunk)`. The hunk string includes its canonical
header and preserves the line endings and no-newline markers in the patch.
Invalid hunk boundaries raise `unidiff.UnidiffParseError`. These patch objects
are read-only; mutation, per-line objects, byte inputs, and metadata-only parsing
are outside this surface.

`itertools.groupby(source, key=...)` groups consecutive items with equal keys. In a
`for key, group` loop, the first statement must assign `list(group)` to a separate
local. That list can be stored or mutated. The grouper name must be fresh and its
only supported use is this immediate conversion.

```python
from itertools import groupby

for title, group in groupby(["alpha", "apple", "beta", "again"], key=lambda word: word[0]):
    words = list(group)
    print(f"{title} {len(words)}")  # a 2, then b 1, then a 1
```

Omitting the key or passing `None` groups by the items themselves. A typed key
function runs once for each item pulled. Filling one group reads the first item of
the following group and keeps its key for the next iteration. Key equality supports
integers, booleans, strings and string enums, bytes, paths, `None`, and fixed tuples
of supported immutable fields.
The compiler rejects source iterators that could keep an open file or a dynamically
bound callback alive through the grouper. A bound key function can own a file
itself and retains it for the lifetime of the last bound group.

## Callable values and callbacks

`typing.Callable[[int, str], bool]` and `collections.abc.Callable[[int, str], bool]` describe the same exact parameter sequence and return type. Module functions, bound/unbound instance methods, class methods, and static methods can be assigned to local variables, passed as arguments, and returned. A bound method owns its receiver, so rebinding the original variable does not invalidate the method, while mutations of that same receiver remain visible. `Callable[[], R]` supports zero arguments. Optional callable locals and parameters require a successful `None` check before invocation.

```python
from typing import Callable


def apply(operation: Callable[[int], int], value: int) -> int:
    return operation(value)


print(apply(lambda value: value * value, 12))
print(sum(map(lambda value: value + 1, filter(None, [0, 2, 3]))))
```

Indirect calls evaluate and retain the callable before evaluating arguments from left to right. They accept positional arguments at full arity: parameter names and default omission are available only on direct calls. Parameter and result types must match exactly; callable variance and implicit numeric conversions are not supported. Functions with keyword-only parameters cannot become callable values.

Noncapturing lambdas obtain parameter types from an expected callable annotation, parameter, or return type, or from the input elements of `map`, `filter`, `sorted`, or `list.sort`. Their bodies are checked against that signature. Captured enclosing locals, `nonlocal`, lambda defaults, variadic parameters, and keyword-only lambda parameters are rejected. `Callable[..., R]` is also rejected. Callable values cannot be stored in imported module globals, class fields, containers, or tuples, including through optional or nested storage. These restrictions keep the supported heap graph acyclic; callable values do not add a cycle collector.

An annotated `def` directly inside another function can read enclosing typed
locals. Each direct call reads their current bindings after evaluating its
explicit arguments. Mutable captured objects retain their identity. A local
helper with positional parameters and no captures can also be a callback,
including in `sorted(map(fingerprint, rows))`. Capturing helpers require a
direct call, and local definitions must be reached before they are used.

`map(callback, source)` accepts one iterable and a one-argument exact callable, in addition to the existing `map(int, source)` conversion. `filter(predicate, source)` truth-tests each callback result and yields the original element; the predicate need not return `bool`. `filter(None, source)` truth-tests the elements themselves and preserves the source element type, including optional types. Both capture their callback and source at construction but invoke callbacks only when pulled. Short-circuit consumers skip later callbacks and failures. Source mutation remains visible under the ordinary list, dictionary, and set iteration rules. Callback errors use the existing exception and cleanup machinery.

## Lazy iteration pipelines

Generator expressions, `zip`, `reversed`, `enumerate`, `map`, and `filter` compose at immediate consumption boundaries: `for`, comprehensions, `list`, `sorted`, string `join`, integer `sum`, `any`, and `all`. The compiler fixes each yielded type and lowers the pipeline into native control flow. These operations do not create a user-visible iterator value; assigning, returning, or passing one to an ordinary function remains unsupported.

```python
def squared_error(expected: list[int], actual: list[int]) -> int:
    return sum((left - right) * (left - right)
               for left, right in zip(expected, actual, strict=True))


def has_positive(values: list[int]) -> bool:
    return any(value > 0 for value in reversed(values))
```

A generator expression evaluates its leftmost iterable and acquires that iterator immediately. Its remaining iterables, filters, and element expression run only as the consumer requests values. Enclosing local bindings remain live between requests; generator targets have private scope. Assignment expressions write to the enclosing scope, but a delayed write does not prove the target initialized after a possibly empty or short-circuited consumer. A top-level `Iterator[T]` function can also be called directly in a loop or comprehension if its final statement is a `for` loop ending in `yield value`. Arguments are evaluated at creation, the producer starts on first pull, and stopping early releases its suspended frame. Yielded values may not own finalizable resources. Escaping generators, other yield positions, `yield from`, and async clauses remain unsupported.

`zip` evaluates its arguments in source order and pulls inputs from left to right for each fixed tuple. It stops at the shortest input by default. Its optional `strict` keyword is truth-tested at construction; a true value raises `ValueError` when exhaustion reveals unequal lengths. Later inputs are probed only when required to detect a mismatch. A consumer that stops early can therefore finish before a mismatch becomes observable. Zero inputs produce an empty iteration.

`reversed` accepts concrete lists, Unicode strings, bytes, ranges, and homogeneous fixed tuples. A reversed list captures its starting index at construction, then reads live elements at descending indexes. Appended tail elements are not visited; an index outside the current list bounds exhausts the iterator when the next item is requested. String traversal yields Unicode scalar strings, and reversed ranges preserve arbitrary-precision bounds. Dictionaries, sets, generator expressions, and other iterator pipelines cannot be passed to `reversed`.

`min(expr for item in source)` and `max(expr for item in source)` consume a single generator of exact integers. They evaluate each yielded value once and keep the first minimum or maximum. An empty generator raises `ValueError`.

`any` and `all` truth-test yielded values and return a boolean. `any` stops at the first true value and returns false for an empty input; `all` stops at the first false value and returns true for an empty input. Skipped elements, conversions, filters, and strict-zip probes do not execute. All source expressions still must type-check.

## Typed JSON boundaries

`json.loads(text)` and `json.load(stream)` accept JSON arrays, objects, and
scalars. `json.loads` accepts strings and bytes; `json.load` reads a text or
binary stream from its current position. Binary input detects UTF-8, UTF-16,
and UTF-32 with or without a byte order mark. A document supports literal string keys, integer indices, length,
truth tests, and iteration. Array iteration yields values and object iteration
yields keys in insertion order. Two parsed values support `==` and `!=`:
list order matters, dictionary key order does not, and numbers compare by value.
Select a value into a concrete scalar or
`dict[str, Any]` at a checked boundary. The latter is a read-only JSON view,
with literal-key `.get()`, scalar or empty-list defaults, and nested iteration.
Original array roots keep their mutable list identity. Malformed JSON raises
`json.JSONDecodeError`, with the original decoded `doc`, short `msg`, character
offset `pos`, and one-based `lineno` and `colno`. The standard decoder uses a heap stack for
deep input and raises `RecursionError` above 50,000 open containers. CPython's accelerator
and pure decoder reach different limits depending on the platform and recursion context.
The explicit typed schema decoder and the encoder retain their 128-container limit. Strings use
Unicode scalar values. Integers retain their exact precision. The standard
reader also recognizes `NaN`, `Infinity`, and `-Infinity`.

`json.JSONDecoder()` and `json.decoder.JSONDecoder()` create a reusable decoder.
Its `decode(s)` method accepts Unicode text; `memo` returns the same dictionary
on each read from that instance. Keys repeated within one document share the
same string object. Separate decodes have independent key namespaces. Like
CPython's default decoder, scanning does not consult or clear entries that a
caller inserts into the public `memo` dictionary. A saved `json.loads` function or bound
`decoder.decode` method can be passed as a callable. Iterating a decoded value
checked as `dict[str, Any]` yields its actual string keys in insertion order;
sorting those keys preserves their owners.

`json.dumps(value)` returns a string and `json.dump(value, stream)` writes to
a text stream. Both accept scalar values, typed lists, tuples, dictionaries
with string or integer keys, and parsed documents. Fresh nested list and dict
displays can mix JSON types. Both functions accept `indent` (integer, string,
or `None`), `ensure_ascii`, `allow_nan`, `sort_keys`, and `separators`.
Separators are unpacked as the item separator and key separator; sorting
compares dictionary keys before converting them to JSON strings.
Defaults match Python's separators, escaping and nonfinite number spellings.
Dynamic hooks, arbitrary mapping keys, and general `Any` remain outside this
boundary.

An array selected into `list[dict[str, Any]]` exposes read-only row views that
can outlive the file. The list supports indexing, iteration, concatenation,
and slicing. Item assignment and mutating list methods are rejected, including
through aliases and function parameters. Lookup checks the row's actual JSON
kind when it is used. Typed scalar or list fields check the selected field's
kind at assignment. Immutable JSON can participate in dataclass equality and
representation. Adding JSON values follows Python's numeric and sequence rules,
and `isinstance(result, int)` can guard the result before storing it as an integer.

`from imre.json import decode, encode` exposes explicit Imre APIs. `decode(text, Schema)` takes a string and a statically resolved record class, `list[T]`, or `dict[str, T]`, returning that exact type. Scalar and optional roots are not accepted by the decoder. Import aliases and first-party re-exports preserve the intrinsic identity. The native decoder constructs existing typed records, lists, and dictionaries directly, without exposing unknown values or `Any`.

```python
from dataclasses import dataclass, field
from imre.json import decode, encode

@dataclass(slots=True)
class Item:
    name: str
    amount: int = 0

@dataclass(slots=True, kw_only=True)
class Report:
    items: list[Item] = field(default_factory=list)

report = decode('{"items": [{"name": "query"}]}', Report)
print(report.items[0].amount)
items = decode('[{"name": "query", "amount": 3}]', list[Item])
print(encode(items, indent=2, ensure_ascii=False))
```

Field annotations define the JSON schema. Supported values are `int`, `float`, `bool`, `str`, `None`, nullable forms, exact lists, string-keyed dictionaries, and nested records with acyclic field types. Dataclass-generated constructors satisfy the structural initialization requirement, including keyword-only and frozen records. An explicit constructor must match every field by name and assign `self.field = field`; it may also replace a defaulted optional list parameter with `[] if field is None else field`. Computed or side-effecting initializers are rejected for decoded classes. Classes used only by `encode` retain their ordinary constructor rules: encoding reads their declared fields without running initialization code. The decoder applies the proved writes directly rather than running arbitrary initialization code. Inside a class method, `decode(text, cls)` uses that method's statically bound class.

A field without a parameter default is required. Scalar parameter defaults apply only to missing keys. Dataclass `field(default_factory=list)` and the explicit fresh-list idiom create separate empty lists for missing fields; explicit JSON `null` still fails a nonnullable list annotation. An optional list with a factory receives `[]` when missing and `None` for explicit `null`. Explicit initializer defaults remain authoritative even when the dataclass field separately declares a default or factory. Missing optional fields are not automatically optional: they need a default to permit absence. Unknown fields are syntax-checked and ignored. Repeated known fields are rejected, including escaped spellings of the same key; repeated unknown fields and duplicates inside ignored objects are ignored.

Decoded dictionaries require exact string keys and reject duplicate keys, including escaped spellings that decode to the same string. Their insertion order follows the input. Non-string dictionary schemas, tuples, sets, bytes, and resource handles are outside the JSON boundary.

Integer fields accept integer tokens of arbitrary precision, excluding booleans, decimal points, and exponents. Float fields accept integer or fractional/exponent tokens, converting to finite binary64; overflow is an error. Integer `-0` converts to positive floating zero, while `-0.0` retains its sign. JSON constants `NaN` and `Infinity`, trailing data, malformed input, schema mismatches, and more than 128 nested object/array levels raise `ValueError`. The nesting limit includes ignored data. Stored strings and decoded record keys must contain Unicode scalar values; ignored values are not materialized. Error wording is not a CPython `json.JSONDecodeError` compatibility promise.

`encode(value, /, *, indent=None, ensure_ascii=True)` returns a string from the same supported value shapes, including scalar and optional roots. It preserves record declaration order and dictionary insertion order. `indent` accepts `None` or an exact integer; zero and negative values insert newlines without indentation. `ensure_ascii` requires an exact boolean. Default compact output uses Python's `", "` and `": "` separators; indented output omits spaces after commas. Integers retain arbitrary precision, finite floats use Python-compatible decimal spelling, and nonfinite floats raise `ValueError`. Encoding enforces the same 128-container nesting limit. It walks concrete storage directly, invokes no user hooks, and does not build an intermediate dynamic JSON tree. The value and keyword expressions evaluate once in source order before encoding starts.

The checked CPython implementation lives in `stdlib/imre/json.py`. Add `stdlib` to Python's import path when running the same source, for example `PYTHONPATH=stdlib python3 program.py` from this repository. The native compiler recognizes the sealed import without executing that module. It rejects a first-party file that shadows the intrinsic namespace.

## Typed TOML boundary

`tomllib.load(stream).keys()` accepts a binary file at its current position. It
reads through EOF and validates the entire document under the TOML 1.0 grammar
used by CPython 3.14. The result can be annotated `collections.abc.Set[str]`.
It supports string membership, length, truth testing, and iteration in source
insertion order after the stream is closed. Invalid UTF-8 raises
`UnicodeDecodeError`. Syntax errors raise `tomllib.TOMLDecodeError`, a
`ValueError` subclass. The native reader bounds TOML nesting at 64 containers.

`from imre.toml import decode` provides `decode(text, Schema)` for complete TOML 1.1 documents. The schema is a statically resolved record class or `dict[str, T]`; TOML's document root is always a table. Import aliases, module imports, and first-party re-exports work as they do for typed JSON. The result uses ordinary concrete records and containers, with no exposed dynamic TOML value type.

```python
from dataclasses import dataclass
from imre.toml import decode

@dataclass(slots=True)
class Group:
    name: str
    href: str

groups = decode('[concepts]\nname = "Concepts"\nhref = "/concepts"',
                dict[str, Group])
print(groups["concepts"].name)
```

Nested schemas support exact `int`, `float`, `bool`, and `str` values, lists, string-keyed dictionaries, fixed records, and their supported optional forms. Records share typed JSON's structural-constructor checks, dataclass support, and native field layouts. Arrays of tables decode as lists of records or dictionaries. Dictionaries retain source insertion order. Bytes, tuples, sets, arbitrary unions, resource handles, and datetime annotations are unsupported.

TOML scalar categories remain distinct: booleans cannot satisfy integer fields, and integer tokens cannot satisfy float fields. Decimal, hexadecimal, octal, and binary integers retain arbitrary precision through Imre's canonical integer representation. Floating-point tokens produce binary64 values, including signed zero, `inf`, and `nan`; overflow follows CPython's conversion to infinity. CPython's maintained parser retains its configured decimal integer conversion limit (4300 digits by default); native decoding has no corresponding digit limit. Applications that need larger decimal inputs under CPython can configure its standard integer conversion guard before decoding.

A missing field requires a constructor default, even when its annotation is optional. Scalar defaults and fresh list defaults follow the typed JSON rules. An optional list with a list factory receives a fresh `[]` when absent; an optional field defaulting to `None` receives `None`. TOML has no null token. Present values must match the optional payload's exact type. Decoding applies the compiler-proved field writes without executing arbitrary constructor code.

The complete document is parsed before schema conversion. Unknown record fields are ignored after validation, and duplicate keys or tables are rejected even in ignored data. Valid date, local time, local datetime, and offset datetime values can appear in ignored fields; they cannot be exposed as strings, records, or dictionaries through a schema. Calendar validation follows CPython's representable range, excluding year zero and leap-second values. Invalid syntax, schema mismatches, missing required fields, and more than 64 nested table/array containers raise `ValueError`; the depth count includes the document table and ignored data. Error wording is not a parser-library compatibility promise.

The CPython module is `stdlib/imre/toml.py`. Add `stdlib` to the import path as for typed JSON. Python 3.15+ uses its TOML 1.1 `tomllib`; earlier versions require `tomli>=2.4`, for example `python -m pip install 'tomli>=2.4'`. Native compilation uses the pinned Rust TOML parser and links no Python interpreter or compatibility module. This boundary currently provides decoding only.

## Native text operations

`ord(value)` returns the Unicode code point of one native string character or the unsigned value of a one-byte `bytes` object. Invalid lengths count Unicode scalars for strings and bytes for `bytes`. Direct consumers can also iterate `map(ord, source)`: input evaluation happens at iterator creation and the conversion runs on each pull. A source `StrEnum` contributes its Unicode value. Mutable `bytearray`, native strings containing lone surrogate code points, and storing the builtin as a first-class callback are outside this subset.

`str.lower()` uses Unicode 16 full lowercase mappings, including expanding characters and contextual Greek final sigma. `str.capitalize()` titlecases the first code point and lowercases the remainder in the original string context.

`str.expandtabs(tabsize=8)` counts Unicode code points, resets the column on CR or LF, and removes tabs for zero or negative widths. Converting the width happens before inspecting the text. Integers, booleans, and fixed classes with an integer-returning `__index__` are supported; values outside the platform C `int` range raise `OverflowError`. A string with no tabs retains its identity. A user `__index__` returning a boolean is rejected because Python's configurable deprecation warning is not supported.

`str.translate(table)` looks up each Unicode ordinal in an exact plain dictionary. A missing ordinal keeps the character; `None` deletes it; an integer selects a code point, and a string can replace the character with any number of characters. An invalid value raises only when its key is hit. Tables use supported homogeneous or optional value layouts, and integer keys or provably distinct string, bytes or tuple keys. An inline ordinal table may mix strings, integers and `None`; the same value layout handles a closed string-or-integer result (such as `logging.getLevelName(name)`). User lookup callbacks, sequence tables, `str.maketrans`, and surrogate output are outside this subset.

The bundled `string` module exports its nine ASCII character sets in CPython order (`ascii_letters`, `ascii_lowercase`, `ascii_uppercase`, `digits`, `hexdigits`, `octdigits`, `punctuation`, `printable`, `whitespace`). `string.capwords(s, sep=None)` accepts native strings, collapses Python whitespace by default, and preserves empty fields with an explicit separator.

`string.Template` supports its default ASCII dollar grammar, a writable string `template`, `substitute`, `safe_substitute`, `is_valid`, `get_identifiers`, and its shared compiled `pattern`. Substitution accepts positional native string-key dictionaries and typed keyword values: None, bool, int, str, bytes, and fixed tuples of these types. Only referenced values are rendered; keywords override mapping entries. Passing `None` as the mapping is distinct from omitting it. Custom mapping callbacks, arbitrary placeholder objects, customized subclass patterns, `Formatter`, `string.templatelib`, and arbitrary module reflection are outside this subset. `Pattern.finditer(text)` streams native `Match` objects for fixed patterns proved to consume input; zero-width patterns are rejected.

The `object` annotation is shared by concrete scalar/tuple values, structured exception fields, and dynamic native module attributes. Metadata and mutable dictionary aliases retain their existing owners.

Native `Mapping[str, object]` is a read-only view of supported concrete dictionaries. It provides string-key lookup, membership, length and truth tests. Typed `**name: T` parameters on a direct source call receive an insertion-ordered dictionary; values are evaluated left to right, and positional-only names can occur as keys. Such functions are not first-class callables in this subset.

A fixed-layout class can inherit from `collections.abc.Mapping[K, V]` or `typing.Mapping[K, V]`. The class supplies `__getitem__`, `__len__` and `__iter__`; a missing primitive makes the class abstract. Membership and `get` use the subclass's lookup, including an override or a raised exception. Both exports accept dictionary instances in class checks. The `typing` alias itself is not a class when passed as the first argument to `issubclass` with the ABC as the second argument.

`str.split(sep=None, maxsplit=-1)` and `str.rsplit(sep=None, maxsplit=-1)` accept both positional and named arguments. With no separator, they group Python whitespace and omit empty fields; an explicit string separator preserves empty fields and must be nonempty. The split limit follows Python's left or right scanning rules. `str.strip([chars])`, `str.lstrip([chars])`, and `str.rstrip([chars])` accept an optional positional string or `None`: omitted/`None` removes Python whitespace, while a string names a set of Unicode scalar values to remove.

`str.startswith(prefix)` and `str.endswith(suffix)` accept one positional string, without tuple alternatives or bounds. `str.find(sub[, start])` accepts a positional substring and an optional integer or `None` start, returning a Unicode scalar index or `-1`; an end bound and `rfind` are not yet supported. `str.partition(sep)` requires a nonempty positional string and returns a fixed `(before, separator, after)` tuple; a missing separator returns `(original, "", "")`. `str.isspace()` is false for an empty string and otherwise checks every character against Python's whitespace definition. These operations use native UTF-8 storage and do not import or call Python methods.

`urllib.parse.unquote(string)` is a sealed native import for one positional or named string argument. It decodes percent escapes with the default UTF-8 replacement behavior. Byte inputs, configurable `encoding`/`errors`, and other URL parsing APIs remain unsupported.

## Datetime and fixed UTC offsets

`datetime.datetime`, `timedelta`, `timezone` and the `UTC` singleton have native immutable
values. A datetime can be naive or use a fixed offset. `datetime.now(tz)` reads the system
clock and requires a native `timezone`; there is no local or DST lookup. Civil construction,
field reads, default ISO formatting, datetime comparisons, shifting a datetime by a
timedelta, and fixed-zone names and offsets are supported. Timedelta construction accepts
int, bool, and float units, normalizes negative durations, and checks overflow.

The [datetime test inventory](../benchmarks/cpython_stdlib/full/datetime/README.md)
lists the operations and original CPython cases checked. This does not change the typed
TOML date boundary.

## CSV reader

`csv.reader(lines)` returns a lazy iterator over lists of strings. The supported
input types are a text file, a live `list[str]`, and a string (one character per
input element). `for`, `list`, `iter(reader)` and `next(reader)` share the
same cursor. `reader.line_num` counts physical lines, including lines pulled
for multiline fields. Errors use `csv.Error`; the default field limit is
131,072 characters. Excel is the supported dialect. Literal `delimiter`,
`quotechar`, `escapechar`, `doublequote`, `skipinitialspace` and `strict`
keywords customize it. Other dialects, writers, dictionary adapters, and
changing the field limit are outside the subset. An explicit text `lineterminator` is validated against the dialect characters; the reader recognizes CR and LF independently of this setting. A nonstring raises `TypeError`.

Use `open(path, encoding="utf-8", newline="")` or `Path.open(..., newline="")`
to preserve original line endings inside quoted fields. The default text reader
recognizes the same physical lines and translates their endings to `\n`.

## Fixed regular expressions

Word escapes (`\w`, `\W`) use Python's Unicode alphanumeric characters plus underscore, including inside character classes.

`re.compile` accepts a literal string and an optional literal `0`, `re.VERBOSE`, `re.UNICODE`, or their bitwise union. Unicode matching is already the default for text.
`re.search(pattern, text)` accepts a literal or a compiled `re.Pattern[str]`.
A compiled pattern has `search(text)` and `match(text)` methods. The latter
starts at offset zero. The compiler checks each
literal, then the executable initializes each distinct translated pattern once.
A module-level declaration before executable statements can be shared by
functions and imported by other modules. Rust’s `regex` and `regex-automata`
crates implement native matching.

`re.sub(pattern, replacement, text)` accepts a literal pattern that cannot match
an empty string and a literal replacement. It replaces every nonoverlapping
match and supports Python's numbered and named capture references, unmatched
groups, and replacement escapes. Callable replacements, counts and flags are
outside the supported subset.

```python
import re

MARKER = re.compile(r"\# \s* E (?: \[ (?P<tag>[^\]]+) \] )?", re.VERBOSE)

def marker(line: str) -> str | None:
    if match := MARKER.search(line):
        return match.group("tag")
    return None
```

Search returns the leftmost match as `re.Match[str] | None`. Every match is
truthy, including an empty match. `group()`, `start()` and `end()` inspect
the whole match and also accept a literal group number or name. `group(0)`
returns a string. A fixed pattern’s required captures return `str`, and its
optional captures return `str | None`. An empty capture returns `""`.
`groups()` returns the capture tuple in declaration order. `groupdict()` returns
a fresh mutable dictionary of named captures. Reading a required literal key
from a local dictionary whose uses preserve its entries also yields `str`.
Offsets count Unicode code points. An unmatched group's offsets are `-1`,
and an unknown group raises `IndexError`. The match retains its original text.

The supported dialect includes literals, dot, simple character classes and
ranges, alternation, named and numbered captures, noncapturing groups, and
greedy or lazy repetition. `\s` and `\S` use Python's Unicode whitespace.
`\d` and `\D` match decimal digits and their complement.
Verbose patterns ignore ASCII layout and comments outside character classes.
Use `\A` or `^` for the start and `\Z` or `\z` for the strict end of the
string. A terminal `$` also matches just before a final newline.
The checker rejects dynamic patterns, bytes, additional flags,
backreferences, lookaround, interior `$`, word shorthands, and repetitions
whose capture or empty-match behavior falls outside this dialect.

## Bounded unions and codec primitives

`str | bytes` and `typing.Union[str, bytes]` denote the same sealed two-member type; either can additionally include `None`. Function, return, local, field, tuple, and container storage enforce those exact members. Concrete strings and bytes promote into union storage without copying or allocating a wrapper. This does not allow arbitrary unions or heterogeneous untyped containers.

`bool | int` stores either boolean singleton or an arbitrary-precision integer, optionally alongside `None`. A mutable field can switch between `False` and `0` without losing the distinction. Both compare equal numerically, but only the boolean is identical to `False`. Tests against `int` accept either member, as in Python.

`isinstance(value, type)` accepts supported concrete type categories and the sealed builtin exception hierarchy. Tests against `str` or `bytes` narrow a local union in the matching branch and exclude that member in the other branch. Reassignments invalidate those facts; mutable field or container reads do not become stable local facts. Custom metaclass tests and runtime type objects remain unsupported.

Integer bit operations preserve Python's arbitrary-precision signed semantics. Negative shifts raise `ValueError`; large right shifts saturate to zero or minus one. A nonzero left shift too large to represent raises `OverflowError`, while zero shifted by an arbitrarily large nonnegative count stays zero. Resource exhaustion may still terminate the process. `divmod(a, b)` evaluates each integer argument once and returns the fixed tuple `(a // b, a % b)`, including Python's signed quotient/remainder rules and zero-division category. Runtime diagnostic wording can differ across CPython releases.

`list.extend(source)` accepts the concrete iteration sources supported by list-display expansion, preserving shallow references and element-type checks. Extending a list from itself captures its original elements rather than iterating newly appended values.

## Truthiness and boolean values

Conditions, comprehension filters, match guards, assertions, `not`, and `bool(value)` use Python's truth rules for the supported exact types. `None`, `False`, numeric zero, and empty strings, bytes, lists, dictionaries, sets, and tuples are false. NaN is true; both floating-point signed zeros are false. Native paths, fixed class instances, exceptions, and file handles are true, including closed files. `bool()` returns `False`. User-defined `__bool__` and `__len__` methods remain outside the subset.

`and` and `or` evaluate operands from left to right and return the selected operand without copying it. Skipped operands do not execute, but still must type-check. A stored or returned result must fit one supported storage type, including the bounded string/bytes union or a nullable form. A truth context can consume differently typed operands directly, as in `if items and len(items):`. Context supplies the element type for empty container operands such as `names: list[str] = [] or ["name"]`.

```python
def display_name(name: str | None) -> str:
    return name or "anonymous"


def first_or_zero(values: list[int] | None) -> int:
    if values:
        return values[0]
    return 0
```

A true optional local is narrowed to its payload. A false optional number or container may still be present, so falsity does not prove `None`; use an explicit None check when that distinction matters. Truth testing never changes storage requirements: assigning an integer or container to a `bool` annotation still fails.

A module can declare an immutable tuple class with `collections.namedtuple("Name", fields)`. Supply a literal field-name string, list, or tuple. Constructors accept positional and named fields and starred fixed tuples; field reads, indexing, unpacking, `_fields`, `__match_args__`, tuple equality for compatible immutable layouts, and named `repr` preserve tuple behavior. An annotated parameter or return must infer the corresponding class and a single concrete field layout. Dynamic factories, declaration options, generated methods such as `_replace`, and subclassing are outside this subset.

A loop can search a fixed tuple of up to eight differently typed fields. The tuple evaluates once before the first test. Imre checks the fresh loop variable at the corresponding field type each time.

```python
for group in (old_diagnostics, new_diagnostics, expected_errors):
    if group:
        return group[0].location.path
```

## Assignment and container expansion

Chained assignment evaluates its RHS once and assigns the same value to targets from left to right. Each target keeps its exact storage type; a declared target can supply context for an empty literal. Shared mutable values retain their aliases:

```python
left: list[int] = []
left = right = [1]
right.append(2)  # left also contains 1 and 2
```

Unpacking accepts fixed tuples, concrete lists, strings, and bytes. Targets may be nested tuple/list patterns containing names, declared fields, or list/dictionary subscripts. One starred target per level collects a fresh shallow list. String elements are Unicode scalar strings; byte elements are integers. The same patterns work in container loop and comprehension targets; range targets still require a name.

```python
sku, quantity = ["widget", "12"]
first, *middle, last = [1, 2, 3, 4]
```

Each level captures all of its values before assigning that level's targets, so a target's mutation cannot change values already extracted. Nested levels are unpacked when their target is reached. A later failure preserves earlier successful stores. A list, string, or bytes length mismatch raises `ValueError`; fixed-tuple arity mismatches remain compile-time errors. A starred tuple remainder must fit one concrete element type, including `T | None`; an empty remainder uses the destination's list annotation when available and otherwise defaults to `list[int]`. Constructors can use chained or nested/starred field assignments while still initializing each declared field exactly once and never reading or exposing an incomplete `self`.

List and set displays accept `*source`, including repeated expansions mixed with ordinary elements. Sources are concrete lists, dictionary keys, sets, strings, bytes, compatible fixed tuples, and the existing `range`, `enumerate`, and dictionary `items()` iteration forms. Dictionary displays accept `**source` from concrete dictionaries. Each expansion completes before the following entry evaluates. Duplicate dictionary keys replace values without changing the key's original insertion position.

```python
values = [0, *[1, 2], 3]
keys = {*{"one": 1, "two": 2}}
merged = {**{"one": 1}, "one": 2, "three": 3}
```

Destination annotations can type empty source literals and promote copied values into optional storage. Existing source containers keep their own exact layout and aliases. A direct call to a source function, ordinary method, classmethod, or staticmethod can expand an exact fixed tuple at any positional argument. Receivers, each starred tuple (including an empty tuple), and keywords evaluate once in Python order. Dynamic iterables and keyword mapping expansion in calls, variadic parameters, and arbitrary user-defined iteration protocols remain outside this subset.

In a function body, a tuple display consumes each `*source` completely before the next expression runs. A later mutation cannot change which elements an earlier spread captured, though mutable elements retain their identity. An iteration error stops the display before the following expression. A homogeneous result can be annotated `tuple[T, ...]` and accepts computed integer indexes. For mixed displays such as `(func, *args)`, a literal index is supported when every possible length gives that position the same type. Starred tuple displays in module initializers, class fields, and parameter defaults remain unsupported.

## Assignment expressions

`(name := value)` evaluates its right-hand side once, stores it in a local, and
returns that same value. The binding has one exact storage type, including an
existing optional type; assigning a concrete value to optional storage does not
make the expression result optional. An argument or container context can supply
the type of an empty literal. Targets must be local names and cannot replace a
fixed function, class, module binding, or builtin.

Assignments follow Python's evaluation order in operands, call arguments,
f-string fields, conditions, slices, and container displays. A skipped boolean
operand, conditional arm, comparison suffix, or assertion message does not
initialize its target. The checker tracks those paths and invalidates optional
facts when an expression may replace the observed value.

```python
def running_totals(values: list[int]) -> list[int]:
    total = 0
    result = [(total := total + value) for value in values]
    print(total)
    return result
```

Inside a comprehension or generator expression, `:=` binds in the enclosing
function or entry scope, including through nested comprehensions. Iteration targets stay private.
Writes already performed remain visible if a later iteration raises. A new
binding is not definitely initialized after a possibly empty comprehension;
initialize it beforehand if it is needed afterward. Assignment expressions are
forbidden in comprehension iterable expressions and cannot rebind any enclosing
comprehension's iteration target, following Python's syntax restrictions.

A named match subject, such as `match (record := parse())`, preserves the target's
declared optional shape and narrows it in matching cases. Rebinding that target
in a guard leaves later cases inspecting the original subject.

## Binary64 numbers

`float` is an exact storage type. It uses IEEE 754 binary64 in native arguments, returns, locals, fields, tuples, lists, and dictionary values. Write `value: float = float(1)` to store an integer as a float. Mixed arithmetic performs Python's integer-to-float conversion and may raise `OverflowError` for an integer too large to convert. Augmented assignment must leave the target's exact type unchanged, so an integer slot cannot use `/=`.

A direct call can pass an exact `int` to a `float` parameter when that parameter appears only in two-operand comparisons with the integer literal `0`. For example, a function that tests `delta == 0` and `delta > 0` accepts integer counts of any size. Imre evaluates the argument once and compares the integer exactly to obtain its sign. Float arguments retain their original binary64 values, including NaN and signed zero. The proof requires at least one comparison and excludes rebinding, captures, and uses of the value elsewhere. Other `float` parameters, calls through `Callable` values, and defaults require exact floats.

Native arithmetic uses no fast-math flags. Signed zeros, infinities, NaNs, and ordinary rounding remain observable. Float division by either signed zero raises `ZeroDivisionError`. Floor division and modulo follow Python's corrected remainder convention, including `1.0 // 0.1 == 9.0` and the divisor's sign on a zero remainder. Overflow in ordinary float arithmetic can produce infinity. Comparisons with NaN are false except `!=`; mixed integer/float comparisons compare exact values without rounding the integer to binary64.

Integer `/` computes the ratio directly and rounds once, including at the subnormal boundary. Neither operand needs to fit in a float: two equal 400-digit integers divide to `1.0`. A result too large for binary64 raises `OverflowError`; underflow preserves the result's zero sign. `int(float)` truncates toward zero, raises `ValueError` for NaN, and raises `OverflowError` for infinity.

`float(str)` accepts signed decimal notation, optional separators between digits, surrounding whitespace, and case-insensitive `inf`, `infinity`, and `nan`. Decimal parsing is correctly rounded; overflow and underflow produce infinity and signed zero. Unicode decimal digits use the pinned ICU character database, whose Unicode version can be newer than a particular CPython build. `float(bytes)` accepts the same decimal grammar restricted to ASCII bytes; non-ASCII bytes raise `ValueError` without Unicode decoding. Malformed input raises `ValueError`. `print`, `str`, and unformatted scalar f-strings use shortest round-trip decimal formatting with Python's decimal tie-breaking, exponent signs, and `.0` for integral values in fixed notation. F-strings also support the bounded fixed-point and percentage formats described below.

Integer addition accepts booleans on either side of `+`, including two booleans. Each contributes zero or one to the integer result. An integer counter can use `total += flag`, including when the counter is a field, list item, or dictionary value.

`abs` accepts exact integers, floats, and booleans. Integer results preserve arbitrary precision, including at the tagged-integer boundary; boolean results are integers. Float results clear the sign, including negative zero and negative infinity, without enabling fast-math behavior.

`float | None` preserves every binary64 bit pattern by boxing only present optional payloads. None remains the nullable zero word; narrowing loads the double while the box is rooted. This costs an allocation when promoting an ordinary float to optional storage, without reserving a NaN payload or conflating absence with `0.0` or `-0.0`. Float dictionary keys, set elements, complex numbers, exponentiation, and float-specific methods remain outside the contract.

## Conditional expressions and sequence access

`yes if condition else no` tests its condition once and then evaluates only the selected arm. Both arms are checked, including dead arms, and must have the same concrete type or fit one supported `T | None`. A typed assignment, argument, or return supplies context to both arms, including empty containers. Local optional narrowing applies separately inside each arm and does not leak past the expression. For example, `value if value is not None else 0` has type `int` when `value` is `int | None`.

String indexing uses an exact `int` and returns a one-code-point string. Negative indexes count from the end; out-of-range indexes raise `IndexError`. Strings remain valid UTF-8, so indexing, slicing and iteration operate on Unicode scalar values, including combining marks and supplementary characters, rather than bytes or grapheme clusters.

`sequence[start:stop:step]` accepts a concrete list, string, or bytes and returns that same type. Typed boundaries supply context through a slice to literal receivers, so `return [][:]` works in a function returning `list[str]`; an existing list cannot change its element type. Each component may be an `int`, `None`, or `int | None`; omission is equivalent to `None`. Booleans are not integer bounds. Bounds and steps may have arbitrary precision, negative steps reverse traversal, bounds are clipped to the sequence, and a zero step raises `ValueError`. In particular, `values[::-1]` and `values[:-1:-1]` have different meanings. List slices create a separate list with shallow copies of the selected element references, preserving alias-visible mutations of contained objects.

The receiver evaluates first, followed by start, stop and step in source order. All remain owned until the read completes; mutations performed by a bound expression affect the slice's current contents and length. A failed evaluation prevents later expressions and the read itself. Tuple slicing, slice objects as user values, string item assignment, and slice assignment/deletion remain unsupported.

String loops, comprehensions, and loop-head `enumerate` capture the original immutable string once and advance by code point in linear time. Each yielded character has its own owner; replacing the source binding or field cannot invalidate the iterator or an earlier character. Repeated arbitrary string indexing may scan UTF-8, so use iteration when visiting every character.

`separator.join(source)` accepts a supported immediate iterable yielding exact strings. The separator evaluates first and remains owned while the source evaluates and is consumed; rebinding its original variable does not change it. Empty input returns an empty string. `text.replace(old, new)` replaces all nonoverlapping occurrences using exact string arguments. An empty `old` inserts `new` at Unicode scalar boundaries, including both ends. Both methods accept positional arguments only; a replacement count and custom string or iteration protocols are unsupported.

## First-party module graphs

For native code, `--extension-path DIRECTORY` adds an installed [Imre extension wheel](extensions.md) to the compile-time search path. Its typed declarations and compiled archive supply the module.

An imported Python module may check its identity before other imports, for example
`if __name__ != "pkg.helper": raise ValueError("unexpected module name")`.
The guard compares a literal string and raises a supported builtin exception
with literal string arguments. If it raises, subsequent dependencies do not initialize.

`imre check` and `imre build` resolve imports under one module root. By default this is the entry file's directory, or the parent of its outermost package when enclosing directories contain `__init__.py`. `--module-root DIRECTORY` selects the root explicitly. An entry inside a package follows `python -m package.program` semantics: enclosing package initializers are loaded and relative imports are valid. A package `__main__.py` can be the entry; `__init__.py` itself cannot.

For example, `import package.operations` binds `package`, `import package.operations as operations` binds the child module, and `from package.operations import solve as run` binds the same function under `run`. Module-qualified calls and imported calls resolve to their defining module's namespace. In an imported module, fixed exports initialized with scalar, list, or tuple literals may precede imports. The compiler initializes those values before the dependency runs; other executable code must follow imports. Imported-module definitions may follow value assignments; calls made during initialization must only reach definitions and globals already initialized at that point, including through methods, constructors, and comprehension helpers. Fixed module, definition, and exported value bindings cannot be reassigned or shadowed by parameters or locals.

Imported modules, including package `__init__.py` files, run their typed top-level code once before their importer continues. Dependencies initialize in import order, and each initializer follows source order. Direct unconditional `NAME = expression` and `NAME: T = expression` assignments create typed exported value bindings. Aliases and re-exports share the same storage, so mutations to an imported dictionary remain visible in the defining module. Chained, unpacked, or conditional initializer locals are not exported. A single-name assignment in the entry module before a later function or class creates a fixed global. Its initializer runs once in source order. Subsequent executable statements use entry locals. A module can import postponed annotations and read its own module docstring through `__doc__`. Initialized global owners are released on normal and exceptional process exits.

An optional `if __name__ == "__main__":` guard without an `else` is supported. Every loaded module is type-checked and lowered for validation, including unused functions and inactive guards. The fixed `__name__` string is `"__main__"` in the entry and the qualified module name in imported code.

Imports are resolved statically from `.py` files and packages with `__init__.py`. Wildcards, dynamic or function-local imports, namespace packages, import cycles, dependencies outside the module root and explicit extension paths, and general standard-library imports or shadowing are rejected. The compiler recognizes sealed bindings for `itertools.groupby`, `enum.StrEnum`, `enum.auto`, `typing.Literal`, `typing.assert_never`, `typing.TextIO`, `typing.BinaryIO`, `typing.Union`, `typing.Callable`, `typing.Self`, `collections.abc.Callable`, `dataclasses.dataclass`, `dataclasses.field`, `pathlib.Path`, `tempfile.TemporaryDirectory`, `subprocess.run`, `subprocess.CompletedProcess`, `subprocess.SubprocessError`, `subprocess.CalledProcessError`, `subprocess.TimeoutExpired`, `os.environ` for `copy()`, `sys.argv`, `io.UnsupportedOperation`, `urllib.parse.unquote`, `imre.json.decode`/`encode`, and `imre.toml.decode`; these do not execute standard-library Python code. Their import aliases and first-party re-exports preserve identity. A package can re-export definitions from its children. A child may import literal exports already initialized by its parent, but cannot read later exports or the unfinished parent's definitions. Symlinks cannot escape the module root or give one source file multiple module identities. The source-string library APIs accept sealed bindings but remain isolated from filesystem imports, which use the path-based APIs.

Closed built-in generic aliases such as `Pair = tuple[str, int]` can be shared between modules. `typing.NewType("Name", str)` can annotate values and its positional constructor returns the original value. Imports and type aliases under a sealed `if TYPE_CHECKING:` introduce annotation-only names.

The I/O surface is intentionally small. `input()` reads a UTF-8 line and removes its trailing LF. A preceding CR remains part of the value, matching CPython's piped input on both hosts. `int(str)` accepts signed decimal digits, optional underscores between digits, and surrounding Python numeric whitespace. Decimal classification is pinned to Unicode 16, matching CPython 3.14; mixed decimal digit families are accepted, while superscripts and other numeric lookalikes are rejected. Other bases remain unsupported. Surrogate escapes in non-raw string literals are rejected: Ruff's analysis AST substitutes replacement characters for them, which is unsuitable for executable semantics. Strings support `+` and local `+=`; `text.split()` groups Python whitespace and omits empty fields, returning `list[str]`. `text.splitlines()` recognizes LF, CR, CRLF, vertical tab, form feed, U+001C through U+001E, U+0085, U+2028, and U+2029. It returns no elements for empty input and no extra element for a final line break; U+001F remains ordinary text. Additional string operations and their accepted signatures are listed under [native text operations](#native-text-operations). `text.splitlines(keepends)` accepts a positional or keyword flag and truth-tests it before scanning, even for empty input. A true flag includes each original line ending (CRLF stays together); a final separator adds no trailing empty item. `text.isspace()` requires a nonempty string and checks Python’s Unicode whitespace characters, including U+001C through U+001F. General formatting protocols and arbitrary codecs remain unsupported. CPython's configurable integer conversion digit limit is not reproduced.

`typing.cast("T", value)` returns the original value; the quoted type adds no runtime check or conversion.

## Function arguments and defaults

An ordinary wrapper can forward `**options` once to a statically known function
that it calls by name. Supply all of that function's positional arguments;
its remaining keyword-only parameters must have immutable scalar defaults.
For example, `def report(value, **options): return format_row(value, **options)`
accepts the same optional named arguments as `format_row`. Caller expressions
still run in source order. Reading, changing, or returning the keyword
dictionary requires general variadic support and is rejected.

First-party functions, instance methods, and constructors accept positional and named arguments. Parameters before `/` are positional-only; parameters after a bare `*` are keyword-only. Other parameters accept either form. Imported aliases and re-exports retain the defining signature. Every call is bound during compilation, including checks for missing, duplicate, unexpected, or incorrectly typed arguments.

```python
def adjust(value: int, /, offset: int = 0, *, label: str = "stock") -> str:
    return f"{label}: {value + offset}"

print(adjust(8))
print(adjust(8, label="orders", offset=2))
```

Defaults must be `int`, `float`, `bool`, `str`, `bytes`, or `None` literals. A number may have one unary `+` or `-`; integers include arbitrary-precision hexadecimal, octal, and binary literals. Ordinary string/bytes-literal rules also apply to defaults. Each default must fit its parameter's exact type: `True` is not an `int` default, and `None` requires `None` or an optional annotation. Mutable containers, tuples, names, calls, arithmetic expressions, and f-strings cannot be defaults, even in an unused function. Literal defaults do not require mutable module state or a general function-object representation.

The receiver of an instance call evaluates first. Supplied arguments then evaluate once in source order, independently of parameter order; a failure prevents later arguments and the callee from running. The compiler applies each parameter's exact type context to its argument, including empty containers and optional values, and retains evaluated values across later mutations and failures. Named arguments lower to ordinary positional native calls.

Calls can expand a fixed tuple literal or local with `*args`, and a dictionary with distinct literal string keys with `**options`. A local dictionary must be assigned once and used only for expansion. Its values are evaluated when the dictionary is created. Rebinding the original variables does not change those values. When a packed value has type `T | None` and the parameter requires `T`, the call checks for `None` and raises `TypeError` before entering the callee. Ordinary nullable parameters still receive `None`.

A function or method can declare homogeneous positional arguments such as `*args: str`. The body can iterate, check the length or expand the values into a fresh list. Indexing and slicing the pack are outside this subset. Returning the pack, mutating it, or observing its concrete storage is unsupported. `**kwargs` parameters, dynamically computed keyword shapes and variable-length spreading into an ordinary fixed call remain unsupported.

Builtins and methods accept specific keywords. `open` accepts `encoding="utf-8"`, `sum` accepts `start`, `zip` accepts `strict`, and both `sorted` and `list.sort` accept `key` and `reverse`.

## String percent formatting

String `%` formatting accepts an exact integer, a string, or a fixed tuple of
positional operands. Tuple items may be `None`, `bool`, `int`, `float`, `str`,
or `bytes`. The entire tuple is evaluated before formatting begins. Width and
precision arguments (`*`) consume tuple items in order.

## F-strings and repr

The string `%` operator accepts an exact integer or string operand. A text
operand supports `%s`, `%r`, `%a`, and a single Unicode character for `%c`.
Literal widths and precisions count Unicode characters. Escaped percent signs
do not consume an operand. A missing argument or a conversion which requires a
number raises a Python exception.

F-strings interpolate exact `int`, `float`, `bool`, `str`, `bytes`, `None`, sealed builtin exception values, fixed records, and lists of values with a supported representation. Ordinary, raw, triple-quoted, and adjacent literals are supported, including escaped braces, Unicode, and escaped NULs. Each field expression and its conversion complete before the next field evaluates. Nested f-strings may appear inside field expressions. Optional values require the same local narrowing as other scalar operations.

```python
def describe(name: str, quantity: int) -> str:
    return f"{name}: {quantity} units"
```

The default conversion and `!s` use scalar conversion or the fixed record’s `__str__` method. Records without that method use their generated representation. A list renders each element with its repr, including for `f"{rows[:5]}"` and `f"{rows!s}"`. `!r` and `repr(value)` additionally format supported containers, tuples, unions, and optional data, recursively composing generated dataclass repr. Strings use Python quoting and escaping. For exceptions, the default conversion and `!s` call their string formatter, and `!r` calls the same formatter as `repr(error)`. This preserves user overrides even in a builtin handler. File-open errors show errno and strerror in repr; UTF-8 decode errors show the encoding, original bytes, half-open bounds, and reason. Resources and callables have no supported repr. The existing restriction against rebinding builtin names still applies. An absent or explicitly empty format specification, as in `f"{quantity:}"`, is supported.

Numeric fields additionally accept literal `f`, `%`, `.Nf`, and `.N%` specifications, with decimal precision `N` from 0 through 1000. Omitted precision is six. All four forms accept a leading `+`, `-`, or space sign option, as in `f"{change:+.2%}"`. Integer and boolean values convert to binary64 using the ordinary checked conversion, so an oversized integer raises `OverflowError` before any later field runs. Fixed-point formatting rounds ties to even and preserves signed zero. Percentage formatting first multiplies the binary64 value by 100 and appends `%`; infinities and NaNs use lowercase spellings. For example, `f"{ratio:.2%}"` and `f"{bytes_used / 1024:.2f}"` are supported. Numeric specifications cannot follow `!s` or `!r`.

Integer and boolean fields support decimal `d` and the `+`, `-`, and space sign options with or without `d`. For example, `f"{change:+}"` produces `+0`, `+3`, or `-3`. Decimal formatting preserves arbitrary integer precision and formats booleans as zero or one.

Integer and boolean fields also accept `x` or `X` with a literal width up to
1000, including zero padding (for example, `f"{code:04x}"`). `chr(int)` returns
a one-character native string for Unicode scalar values. Out-of-range integers
raise `ValueError`; isolated surrogate code points cannot be represented by a
native string.

Dynamic specifications, width, alignment, other numeric presentation types, `!a`, and debug `=` fields are rejected, including `f"{quantity=!s}"`. Records support a statically resolved `__str__` method. Custom `__format__` and `__repr__` methods are unsupported.

Raw prefixes apply only to their own literal text. Expressions inside a raw f-string still follow their ordinary string rules, and adjacent parts keep their individual prefixes. Surrogate escapes in non-raw literal text or field expressions are rejected consistently with ordinary strings; tabs and form feeds inside literal text remain data.

A string template can substitute named `str` and `int` arguments, as in `url.format(filename=file, line=42)`. The template may be computed at runtime. Arguments evaluate once in call order, including unused arguments. Repeated fields reuse their value and `{{` and `}}` produce literal braces. An empty specification and `!s` are supported. A missing name raises `KeyError` and an unmatched brace raises `ValueError`. Positional call arguments, field traversal, and the rest of the format mini-language are outside this subset.

`textwrap.dedent(text)` removes the common prefix of spaces and tabs on nonblank lines. Each tab counts as a distinct character. Whitespace-only lines become empty as in Python 3.14. Interpolation happens before dedenting, so newlines in field values participate in the common-prefix calculation.

`textwrap.indent(text, prefix, predicate=None)` preserves line endings, including CRLF and Unicode line separators. By default it prefixes lines that contain a non-whitespace character. An optional `Callable[[str], bool]` receives each complete line, including its terminator. Errors raised by the predicate stop iteration.

`textwrap.TextWrapper`, `wrap`, `fill`, and `shorten` support string input, explicit wrapping options, and mutation of a wrapper between calls. Word splitting follows CPython's whitespace and hyphen rules, including its Unicode word characters. Boolean options also accept integers; when choosing how to split, CPython checks whether `break_on_hyphens` is the singleton `True`. A dynamically assembled keyword dictionary still needs the compiler's normal keyword-unpacking support.

## Immutable bytes

`hashlib.sha256(data=b"", *, usedforsecurity=True)` constructs a mutable SHA-256 hash from exact bytes. `update(data)` adds bytes and returns `None`. `digest()` returns 32 bytes and `hexdigest()` returns 64 lowercase hexadecimal digits; neither prevents further updates. An assignment shares the same hash, while `copy()` creates an independent state. The read-only attributes are `name`, `digest_size`, and `block_size`. Both values of the boolean `usedforsecurity` select the same SHA-256 implementation. The named constructor `hashlib.new(name, data=b"", *, usedforsecurity=True)` resolves SHA-256 aliases at runtime. `hashlib.algorithms_available` is a shared mutable set of the provider’s supported canonical names (currently `sha256`); adding a name to the set does not install a new algorithm. Unknown names raise `ValueError`. The immediate `sha256(data).hexdigest()` spelling can hash the bytes directly without allocating a mutable object.

`bytes` is an exact immutable type distinct from `str`. Ordinary, raw, escaped, and adjacent byte literals use Python's literal rules. Every byte from 0 through 255 is valid, including NUL and invalid UTF-8. Bytes can appear in classes, tuples, lists, dictionary keys/values, sets, and optionals. Literal bytes are also accepted as immutable parameter defaults.

`bytes()` returns empty bytes; `bytes(count)` creates that many zero bytes; `bytes(list_of_ints)` checks each integer is in 0 through 255; and `bytes(existing_bytes)` preserves its value. Counts must fit a native signed index before the nonnegative check: negative representable counts raise `ValueError`, unrepresentable counts raise `OverflowError`, and out-of-range list values raise `ValueError`. Booleans do not satisfy these exact integer inputs. Other iterable constructors and the string-plus-encoding constructor are outside this milestone.

For `needle in data` and `needle not in data`, a bytes needle searches for a contiguous subsequence (an empty needle always matches). An integer or bool tests for one byte. Integers outside 0 through 255 raise `ValueError`, even for empty data. Exact `None`, text, float, list, tuple, dictionary and set needles raise `TypeError`. User-defined index or buffer protocols and mutable buffer exporters such as `bytearray`, `memoryview` and `array.array` are outside this subset.

Indexing uses exact integer indexes, including negative indexes, and returns an `int` in 0 through 255. Out-of-range indexes raise `IndexError`. Slicing follows the sequence rules above and returns bytes. Iteration, comprehensions, and loop-head `enumerate` yield integers in byte order while retaining the original immutable source independently of rebinding. Concatenation requires two byte values; repetition accepts bytes and an exact integer in either order. Repetition converts the count to a native signed index even for empty input and rejects an overflowing result. All six comparisons use unsigned-byte lexicographic order. `print`, `str`, and scalar f-strings use Python's `b'...'` representation, including quote selection and hexadecimal escaping.

`base64.urlsafe_b64encode(data)` accepts immutable bytes and returns padded bytes using the URL-safe alphabet.

`text.encode(encoding="utf-8", errors="strict")` accepts positional and named string arguments. UTF-8, UTF-8 with a signature, ASCII and Latin-1 use the native codec registry's implementations. For example, `"¢".encode("ascii", "backslashreplace")` yields `b"\\xa2"`. ASCII and Latin-1 also support `strict`, `ignore`, `replace` and `xmlcharrefreplace`. Native strings contain Unicode scalar values.

`data.decode()` performs strict UTF-8 decoding. Its optional positional encoding may be a literal UTF-8 spelling; `data.decode("ascii")` supports strict ASCII and reports the first non-ASCII byte and its position. Decode errors raise `UnicodeDecodeError`, including Python's byte positions/ranges and invalid-start, invalid-continuation, or truncated-sequence message. Mutable `bytearray`, memory views, implicit text/byte conversions and other byte methods are outside this subset.

## Native paths and directory traversal

On Linux and macOS, `stat` exposes the POSIX mode constants, `S_IFMT`, `S_IMODE`, the file-type predicates, and `filemode`. These accept positional integer modes, including arbitrary-precision inputs: the accelerated CPython rules raise `OverflowError` when a value does not fit the platform's unsigned mode type. Direct calls also accept booleans. Darwin's whiteout type and file flags use Darwin values. [CPython parity and exact test coverage](../benchmarks/cpython_stdlib/full/stat/README.md).

On POSIX, `os.devnull` is `"/dev/null"`. `os.path.commonpath(paths)` compares path components for lists of `str`, `bytes`, or native `Path` objects and literal list or tuple displays. It removes redundant separators and `.` components, preserves `..`, and rejects an empty sequence, mixed string and byte paths, or mixed absolute and relative paths.

On POSIX, `os.path.join(a, *parts)`, `basename(p)`, `dirname(p)`, and `splitext(p)` take strings, bytes, or native `pathlib.Path` values. Calls can also use `posixpath`, aliased modules, and imported function names. String and byte spellings preserve redundant slashes, dots, and trailing separators according to each function. A native `Path` supplies its normalized string spelling. A join accepts fixed positional components and raises `TypeError` when string and byte paths are mixed. `splitext` returns a pair of the input string or byte type.

`os.path.normpath(path)` removes redundant separators and dot components and collapses `..` lexically. Exactly two leading slashes survive. `os.path.abspath(path)` applies the same rules after prepending the current directory to a relative input; it does not resolve symlinks or require the path to exist. Both accept `str`, `bytes`, and native `Path`. Byte inputs produce bytes even when a path or working directory contains invalid UTF-8. Text results must be representable as Imre strings.

`from pathlib import Path` exposes a sealed native POSIX path type on both supported hosts. `Path()` represents the current directory; positional strings, native paths, and supported `os.PathLike` instances join left to right. Arguments are evaluated before conversion; an instance must return a string from `__fspath__`. Bytes and invalid protocol results raise `TypeError`. The `/` operator accepts a path and a string or path, including a string on the left. An absolute segment replaces the preceding path. Empty segments, repeated separators, and `.` components normalize lexically; `..` components and exactly two leading slashes remain intact. Construction and joining do not access the filesystem or resolve symlinks. `path.joinpath()` returns a fresh equal path. It accepts the same segments as the constructor. `path.with_segments()` constructs a fresh path from its supplied positional segments.

`PurePath` and `PurePosixPath` select POSIX lexical rules. `PureWindowsPath` handles Windows drive-relative paths, rooted paths, UNC share names and device prefixes on either host. Its equality and dictionary keys ignore case; paths of different flavors compare unequal. Nested paths of the same flavor preserve their original segments. Crossing flavors uses the other path's forward-slash spelling. Pure paths do not access the filesystem. All path types expose `drive`, `root`, and `anchor`.

`name`, `stem`, `suffix`, and `parent` are read-only properties. Parent traversal stops at the anchor or current directory. Components follow [Python 3.14 semantics](https://docs.python.org/3.14/library/pathlib.html), including `Path("report.").suffix == "."`. On these POSIX hosts, `path.as_posix()` returns the same text as `str(path)`. Paths compare by normalized value with `==` and `!=`; `str`, `print`, and ordinary f-strings produce the path text, while `repr` and `!r` produce `PosixPath(...)`. Paths are always true, including `Path("")`. They can appear in exact fields, lists, dictionary keys and values, sets, tuples, callables, and nullable storage. Dictionary and set lookups hash the normalized path spelling without accessing the filesystem. Dataclass equality and repr traverse their native values. Path ordering, subclasses, and arbitrary `os.PathLike` implementations are outside this contract.

```python
from pathlib import Path

root = Path("reports")
for path in sorted(root.glob("*.json"), key=lambda path: path.name):
    print(path.stem)
    print(path.read_text())
```

`exists()`, `is_file()`, and `is_dir()` follow symlinks and return false for failed filesystem queries, matching Python 3.14, including invalid NUL-containing paths.

`Path.stat(*, follow_symlinks=True)` and `Path.lstat()` return a native
`os.stat_result`. Its ten tuple entries include integral timestamps; the
named time attributes include fractions, and their `_ns` counterparts retain
nanoseconds. Linux and macOS also expose block size, blocks and device type.
macOS exposes birth time, file flags and generation. OS failures retain the
original path spelling in `OSError.filename`.

`Path.samefile(other_path)` follows symlinks and compares inode and device
numbers. It reads the receiver's metadata first. For a typed operand with a
`stat` method, it calls that method; a missing attribute or an `AttributeError`
during the call triggers path conversion and a filesystem query. A different
inode returns false without reading the operand's device number.

`Path.read_text(encoding=None, errors=None, newline=None)` and `write_text(data, encoding=None, errors=None, newline=None)` use the same native file backend as binary reads and writes. UTF-8 is the default; UTF-8 with signature, Latin-1 and ASCII are also supported. Strict, ignore, replace and backslash-replace error modes work for these encodings; single-byte writes also support XML character references. With no newline override, reads translate CRLF and CR to LF. Empty or explicit newline strings keep the input line endings; on POSIX, write-side CR and CRLF overrides replace each LF. Writes return the number of input Unicode scalars, independent of encoded length. Data of the wrong type raises `TypeError` before opening the target. An invalid newline or codec name on an otherwise valid write is diagnosed after opening and truncating the file.

`mkdir(mode=0o777, parents=False, exist_ok=False)` creates a directory. The mode is an exact integer within the signed C-int range; larger values raise `OverflowError`. Boolean options require exact booleans. The process umask applies, and missing parents use mode `0o777` independently of the leaf mode. `exist_ok=True` accepts an existing directory, including a symlink to one, without changing its permissions. `unlink(missing_ok=False)` removes a file or symlink; its flag suppresses only `FileNotFoundError`. `rmdir()` removes an empty directory. These methods return `None` and preserve their normal filesystem error categories.

On POSIX, `new_link.symlink_to(target, target_is_directory=False)` and `new_link.hardlink_to(target)` accept strings, bytes, native paths, and typed `__fspath__` methods returning strings or bytes. Hard links follow a symlink source. Relative symlink targets retain their original spelling on disk; `readlink()` returns that target as a fresh Path without following it. OS failures retain the two converted pathnames. Reading a target containing undecodable bytes still needs surrogateescape support. An OS error involving a `StrEnum` source currently retains its text, losing the enum identity.

Concrete paths expose `info`, a view with `exists()`, `is_dir()`, `is_file()`, and `is_symlink()`. The first three accept the keyword-only `follow_symlinks` flag. A saved view retains its results after filesystem changes; a new `Path(p)` starts with an empty cache. Paths from `iterdir()` use the file type reported by the directory scan. Calling these predicates on a path directly queries the filesystem again.

`absolute()` prefixes a relative path with the working directory and preserves symlinks and `..` components. `is_absolute()` checks the lexical anchor without accessing the filesystem. `resolve(strict=False)` returns a new absolute path, resolves symbolic links, and removes `..` components in filesystem order. Its `strict` argument is an exact boolean and may be positional or named. With `strict=True`, missing components, non-directory traversal, permission failures, and symlink loops retain their operating-system exception categories. The default resolves as far as possible, appending unresolved components while still normalizing parents. Acyclic symlink chains are not limited by the kernel's per-system-call symlink count. Argument evaluation precedes filesystem access. Resolution requires UTF-8 paths and symlink targets.

`is_relative_to(other)` and `relative_to(other, *, walk_up=False)` compare lexical components and anchors. The Windows flavor compares components without case; POSIX distinguishes `/` from `//`. Walking up cannot cross anchors or climb over an unmatched `..` in `other`. `with_name`, `with_stem` and `with_suffix` return new paths of the receiver's flavor. The `suffixes` property returns a fresh list, including trailing single-dot suffixes.

`with_name(name)` replaces the final component with one nonempty string other than `.`. A slash in the new name or an empty name on the original path raises `ValueError`.

`Path.open()` defaults to a UTF-8 text reader. It accepts literal read, write, append, and exclusive-create modes in text or binary form, with an optional literal `encoding="utf-8"` for text. It uses the same stream operations and context manager cleanup as builtin `open`. Update modes, custom buffering, newline and error handlers are outside the native subset. `read_bytes()` and `write_bytes(data)` read or write exact bytes without decoding. They close the stream before returning, including reporting a buffered output failure. `write_bytes` returns the number of bytes written.

`replace(target)` accepts a string or native path, performs POSIX replacement, and returns the destination `Path`. Relative targets refer to the process working directory. The original path value remains unchanged. A string target reaches the filesystem before its returned path is normalized, so a trailing slash retains its operating-system meaning. Existing files or empty directories can be replaced, subject to normal type and filesystem restrictions; cross-filesystem replacement fails. The rename is atomic on one filesystem, but does not promise durable storage. All mutation arguments evaluate in source order before the operation; positional and keyword calls follow the corresponding Python signatures.

`Path.match(path_pattern)` matches relative patterns from the right and anchored patterns against the whole path. `Path.full_match(pattern)` matches the entire normalized spelling and allows a `**` component to span directories. Both take strings or native paths and accept the keyword-only `case_sensitive` argument. `None` uses the POSIX case-sensitive default. These operations require no filesystem access.

On POSIX, `iterdir()`, `glob(pattern)`, and `rglob(pattern)` return iterators of native paths. They can be stored, shared, advanced with `next`, or consumed by loops and iterator pipelines. The glob methods take strings or native paths. They support multiple components, `*`, `?`, bracket expressions, recursive `**`, and a trailing slash for directories. Matching includes leading-dot names. The keyword-only `case_sensitive` flag overrides the platform default; `recurse_symlinks=True` follows directory symlinks when expanding `**`. Empty patterns raise `ValueError` in `glob`, and anchored patterns raise `NotImplementedError`.

Directory acquisition follows Python 3.14: calling `iterdir()` immediately snapshots directory entries and closes the scan, while `glob()` defers filesystem access until its consumer requests a value. A wildcard glob snapshots and closes the directory scan before yielding its first result. Later body mutations do not extend that snapshot. `iterdir` propagates scan failures; `glob` suppresses operating-system scan failures, but other errors retain their normal category. Hidden traversal owners are reclaimed after exhaustion, early exit, and exceptions.

Native strings contain Unicode scalar values and paths use UTF-8. A directory entry with undecodable filename bytes raises `UnicodeDecodeError`; Unix surrogateescaped filenames are outside this domain. Lexical operations retain embedded NULs, while accesses that require a valid OS path raise `ValueError` unless the predicate or glob behavior above suppresses that particular failure. No operation silently substitutes replacement characters.

## Scoped temporary directories

[`tempfile.TemporaryDirectory`](https://docs.python.org/3.14/library/tempfile.html#tempfile.TemporaryDirectory) is supported directly in a `with` item. Its keyword-only options are `dir=None`, `prefix="tmp"`, and `suffix=""`. The directory accepts a string, native path, or nullable form; prefix and suffix require exact strings. The optional `as` target is one string name. Import aliases and first-party re-exports retain the sealed constructor identity.

```python
from pathlib import Path
from tempfile import TemporaryDirectory

output = Path("report.md")
with TemporaryDirectory(dir=output.parent, prefix=".imre-") as name:
    staged = Path(name) / "report.md"
    staged.write_text("Complete report\n", encoding="utf-8")
    staged.replace(output)
```

Creation chooses an unused name, requests mode `0o700` subject to the process umask, and returns an absolute pathname. Prefix and suffix retain ordinary POSIX joining behavior, including separators. An omitted directory uses the first usable candidate from `TMPDIR`, `TEMP`, `TMP`, the standard temporary locations, and the current directory; this choice is cached for the process as in CPython. Creation failures propagate before the body starts.

Cleanup removes the directory tree on fallthrough, return, loop transfers, and exceptions. A hidden owner retains the original pathname even if the visible target is rebound. Multiple file and workspace managers enter left to right and clean up in reverse order; failure to create or enter a later manager still cleans up earlier ones. A cleanup error replaces an earlier exception or pending return, while outer cleanup continues. Returned or stored strings remain ordinary values after the directory is removed.

Missing entries and an already-removed root are tolerated. Descendant symlinks are unlinked without following them; a root replaced by a symlink or regular file raises an error. Cleanup repairs owner permissions where CPython does and traverses raw Unix entry names without decoding them into language strings. This permits removal of an opaque filename created by an external process while preserving the Unicode boundary for public path operations. Moving the entire root elsewhere leaves that moved tree outside the original scope's cleanup path.

Construction creates the directory immediately. A local can hold the manager before entering it. A normal or exceptional exit recursively removes the directory. If the last owner goes away before entering, cleanup is best effort. A conditional can choose between `TemporaryDirectory()` and `contextlib.nullcontext(path)`. The temporary branch yields its string name and the other branch yields the supplied `Path`. Passing that value to `Path(...)` accepts either representation. Constructor options remain limited to `dir`, `prefix`, and `suffix`.

### Filename patterns

The POSIX `fnmatch` module supports `fnmatch`, `fnmatchcase`, `filter`, `filterfalse`, and `translate`. Match filenames and patterns as strings or as bytes. A slash or leading dot is an ordinary character. The filters accept supported iterables, keep the original items in order, and raise `TypeError` at the first str/bytes mismatch. See the [complete CPython fnmatch cases and native results](../benchmarks/cpython_stdlib/full/fnmatch/README.md).

## UTF-8 files and process arguments

`open(path)` and `open(path, "r")` read text using strict UTF-8. `open(path, "w")` creates or truncates the file and writes UTF-8. All three accept an explicit `encoding="utf-8"`. The path can be a string or a native `Path`. Each call returns a sealed `typing.TextIO` handle. The handle can be passed, returned, or stored in a field or container. All aliases observe the same open or closed state.

`read()` consumes the remaining file and decodes strict UTF-8, translating CRLF and bare CR to LF. Further reads at EOF return the empty string. `write(text)` writes UTF-8 and returns the number of Unicode scalar characters, including embedded NULs. `flush()` makes buffered output visible to the operating system; it does not promise durable storage. `close()` flushes and closes, marks the shared handle closed even on a close failure, and succeeds on repeated calls. `.closed` is read-only. Entering, reading, writing, or flushing a closed handle raises `ValueError`. Reading a write-only handle or writing a read-only handle raises `io.UnsupportedOperation`, which matches both `OSError` and `ValueError`.

`open(path, "rb")`, `open(path, "wb")`, `open(path, "ab")`, and `open(path, "xb")` return a sealed `typing.BinaryIO` handle. Binary calls accept no encoding or other keyword arguments. Read mode opens an existing file, write mode creates/truncates, append mode creates if absent and appends, and exclusive-create mode raises `FileExistsError` if the path already exists. `read()` returns all remaining raw bytes without newline conversion; `write(bytes)` returns the number of bytes written. EOF returns `b""`. Binary handles support the same shared ownership, `flush`, `close`, `.closed`, and context cleanup as text handles. Their access mode survives closure: reading a write-only stream or writing a read-only stream raises `io.UnsupportedOperation` even after it closes; supported operations on a closed stream raise `ValueError`.

`with` accepts these builtin handles and the temporary-directory scopes above, with optional name-only `as` targets and multiple managers. It evaluates and enters them left to right and cleans them up in reverse order. Cleanup runs on fallthrough, errors, returns, and loop transfers. A hidden owner retains the entered file independently of any rebinding of its visible target. A later manager's failure closes earlier files and removes earlier workspaces. A close failure replaces an earlier exception or return, while outer cleanup still runs. The target remains bound after normal exit, usually to the now-closed file.

Both reader types support `readline()` and line iteration. Text reads incrementally decode strict UTF-8 and translate CR, LF, and CRLF to LF, even across buffer boundaries. Binary lines retain their bytes and end at LF. A line includes its terminator when one exists. An empty read signals EOF, so a blank line and the last unterminated line are both observable. Aliases, `for` loops, comprehensions, and explicit reads advance the same cursor. Breaking a loop leaves the handle available to its owner. Binary `read(n)` accepts an exact integer, reads at most that many bytes, and returns the remainder for `-1`. Zero requests no bytes. Smaller negative sizes raise `ValueError`, and sizes outside the native signed index range raise `OverflowError`.

`gzip.open(path, "rb")` and `gzip.open(path, "rt", encoding="utf-8")` accept a string or native path and expose the same buffered read operations. Concatenated gzip members form a single stream. Consuming a malformed header or trailer can raise `gzip.BadGzipFile` (an `OSError`), a truncated member can raise `EOFError`, and invalid compressed data can raise `zlib.error`.

`zipfile.ZipFile(path)` opens a ZIP archive for reading from a string or native path. The mode may be omitted or given as the literal `"r"`. Each `namelist()` returns a fresh list of names in central-directory order, preserving duplicate names without inflating file contents. A `with` statement closes the archive on all exits; aliases share the descriptor, `close()` is idempotent, and the list of names remains available after close. Invalid directories raise `zipfile.BadZipFile`.

Read-only archives also accept a seekable binary file or `io.BytesIO([initial_bytes])`. `infolist()` and `getinfo(name)` expose ordered `ZipInfo` metadata; opening a duplicate by `ZipInfo` selects that entry, while a filename selects the last occurrence. `read(name_or_info)` returns bytes. `open(name_or_info[, "r"])` exposes a binary member reader that streams stored and DEFLATE data and checks CRCs as it reads. Independent readers can outlive the archive. `testzip()` returns the first corrupt member name or `None`. Caller-owned binary files remain open after closing their archive; the binary reader supports `seek(offset[, whence])` and `tell()` on seekable inputs.

Final reference release closes a file best-effort without callbacks or a new pending exception. Use explicit `close` or `with` to observe close errors. Text append modes, update modes, text seeking, sized text reads, gzip writing, arbitrary codecs, newline options, generic context-manager protocols, and user finalizers are outside this contract.

Invalid UTF-8 raises `UnicodeDecodeError`, matching `UnicodeError` and `ValueError`. Embedded NULs in paths raise `ValueError`. Missing files, permission failures, directories opened as files, and non-directory path components use `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`; exclusive-create collisions use `FileExistsError`, also an `OSError`; other operating-system errors use `OSError`. Runtime-generated message text is not a CPython compatibility promise.

`sys.argv` and `sys.orig_argv` are independent mutable lists, each shared across aliases and modules. In a standalone executable both start with the arguments passed to its main function. With `imre run`, `sys.argv` starts with the source path followed by the program arguments after `--`; `sys.orig_argv` contains the actual Imre command, flags, and program arguments. Element writes and list mutation are supported; rebinding a module attribute or imported name is not. Native arguments must be valid UTF-8. Invalid Unix argument bytes raise `UnicodeDecodeError` when the list is first accessed; CPython's surrogateescaped strings are outside this runtime's string domain. Programs that never read process arguments emit no argument initialization or shutdown calls.

`imre [-I] [-B] [-O ...] [-X utf8[=0|1]] -c CODE [ARG ...]` compiles the command in memory and runs it as `__main__`. Python 3.14's command dedent applies before parsing. The program sees `['-c', *args]` in `sys.argv`; `sys.orig_argv` preserves the complete invocation, including the original indentation and option spelling. `sys.executable` identifies the Imre launcher. Commands may import supported source modules from the working directory and `PYTHONPATH`. `-I` excludes the working directory and ignores `PYTHONPATH` and other Python startup environment flags. The selected `isolated`, `ignore_environment`, `safe_path`, `no_user_site`, `dont_write_bytecode`, and `utf8_mode` fields are available through `sys.flags` in command mode. The native compiler's existing text, filesystem, and import restrictions still apply. `IMRE_ENCODED_FLAGS` and `IMREFLAGS` select the native compiler and linker settings independently of Python startup flags.

In command mode, each `-O` raises `sys.flags.optimize` by one. `PYTHONOPTIMIZE` can raise that level unless `-I` ignores it. At level one or higher, `__debug__` is false and assertions do not evaluate their conditions or messages. At level two or higher, the module docstring is `None`. File builds use level zero, including when LLVM code generation uses `--opt-level 3`.

## Command-line options

A function can declare an `argparse.ArgumentParser`, add required positional arguments and named options, and return the result of `parse_args()`. An explicit `list[str]` supplies the arguments to parse; omitting it or passing `None` reads `sys.argv[1:]`. Imre derives a read-only namespace with a concrete field for each option. Hyphens become underscores in field names. A choice of strings gives the field a string `Literal` type. A value option without a default or `required=True` produces an optional field.

```python
from argparse import ArgumentParser
from pathlib import Path

def options():
    parser = ArgumentParser(description="Check a file")
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--mode", choices=["brief", "full"], default="brief")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--verbose", action="store_true")
    return parser.parse_args()

args = options()
if args.output:
    args.output.write_text(args.input.read_text())
```

Positional and named arguments accept `nargs="?"`, `"*"`, `"+"`, or a positive integer. A fixed count (including `1`), `"*"`, or `"+"` produces a list. An earlier positional reserves enough values for the following positionals. A named option with `nargs="?"` uses its `const` when present without a value and its default when omitted. Named options accept one or more ASCII spellings, such as `-v` and `--verbose`, with `required` or `action="store_true"`. A list, tuple or `[member.value for member in Mode]` over a sealed enum supplies constant string choices. Omitted options with a nominal enum member as the default keep that member. Explicit values are strings. Constructor options include a literal `prog`, a literal description or `__doc__`, and `RawDescriptionHelpFormatter`. In the default configuration the displayed program name is the entry script's basename. The parser reads the current `sys.argv[1:]` and handles repeated options, unique long-option abbreviations, and `--`. Help layout and coloring follow CPython 3.14. Help and usage errors follow the native exception cleanup path and raise catch-only `SystemExit` with status 0 or 2. The `BooleanOptionalAction` class adds a `--no-` spelling for each long option, accepts short aliases, and stores the value of the last occurrence.

Arguments accept the built-in `int` and `float` converters. Integer arguments also support fixed or variable length lists. Literal numeric choices compare converted values; string choices may be a single string of characters, and integer choices may be a literal `range`. Failure to convert an argument reports that option and its original value on stderr. String defaults and constants are converted when used. An integer default or constant on a string argument retains its integer type.

Set `prefix_chars` to declare alternate option prefixes. The help flags follow the chosen prefix; `add_help=False` disables them and lets an application reuse their spellings. `allow_abbrev=False` disables single- and double-prefix abbreviations while preserving short-option grouping. Options may set a literal `dest` independently of their spellings.

Call `format_help()` or `format_usage()` to obtain a string, including its final newline. `print_help()` and `print_usage()` write to stdout by default and also accept stderr or a native text file. A declared parser can be used for more than one of these operations or parse calls.

Use `group = parser.add_argument_group(title, description)` to place arguments in a separate help section. Arguments added to a group bind in the same order and populate the same namespace as arguments added directly to the parser. A parser or titled group may create a mutually exclusive group. Repeating one member is allowed; selecting a different member reports both actions in the error. A required group needs an explicitly supplied member, even when another member has a default.

A subcommand is optional by default. With `add_subparsers(dest="command")`, the namespace contains `command=None` when omitted. Without `dest`, no command field is added. Use `required=True` or set the group's `required` attribute to require a selection. Each literal `add_parser("name", aliases=["alias"], help="...")` introduces a child parser with its own options. The destination retains the spelling the caller used; child help uses its canonical name. A `match args.command` case or `if args.command == "alias"` branch selects the child's concrete layout. A directly called handler may annotate that argument as `argparse.Namespace`.

For literal option declarations, a keyword outside the constructor signature of
`BooleanOptionalAction`, `store_true`, `store_false`, `store_const`, `append_const`,
or `count` raises `TypeError` at the `add_argument` call. An explicit `--no-` spelling on
`BooleanOptionalAction` raises `ValueError`. Keyword expressions run before the
constructor error. If the caller catches it, previously registered options remain usable.

Keep each builder local to a function or method. Declare parent options first, followed by each subparser and its options, then call `parse_args()` or `parse_known_args()`. The latter returns the namespace and a list of unrecognized arguments in order. An explicit list or tuple literal supplies the arguments; a standalone call still checks them and can raise `SystemExit`. Dynamic declarations and nested subcommands are outside this grammar.

The `store_true`, `store_false`, and `store_const` actions store a value without consuming the next argument. A `count` action increments once for each occurrence, including each letter in a cluster such as `-vvv`.
An unshadowed builtin class such as `object` or `int` can be a `store_const` value or its default. The namespace field contains the class itself.

## Standard logging


`logging.LogRecord` and `makeLogRecord` retain named scalar and container fields. `getMessage()` formats positional and mapping arguments when called. A record captures its creation time and thread/process metadata; changes to `logging.logThreads`, `logProcesses` or `logMultiprocessing` affect subsequent records. An annotated scalar read can fetch an extra field (for example, `request_id: str = record.request_id`).

`logging.getLogger(name=None)` returns the same native logger for each name. Omitted, None, empty, and `"root"` names refer to `logging.root`. Loggers expose `name`, `level`, `root`, `parent`, `setLevel(int)`, `getEffectiveLevel()`, and `isEnabledFor(int)`. Named levels inherit through dot-separated parents. Integer levels retain Python's full precision.

`debug`, `info`, `warning`, `error`, `critical`, `fatal`, and `log(level, message, ...)` write to standard error. Scalar message arguments support literal `%s`, `%r`, `%d`, and `%%`. Arguments execute in source order and conversion waits until the event is enabled. The fallback handler emits warning and higher levels before configuration. The module-level convenience functions configure the root handler on first use.

`logging.basicConfig` accepts `level`, `format`, `datefmt`, and `force`. A configured handler keeps its settings unless forced. Both configuration and `setLevel` resolve string levels in the native registry. `addLevelName` updates the integer label and the reverse name lookup; `getLevelNamesMapping()` returns a fresh dictionary. `getLevelName(number)` returns a string. For a string argument, use `isinstance(result, int)` to distinguish a registered name from the fallback text. The percent-style format accepts `name`, `levelname`, `levelno`, `message`, and local-time `asctime`. Custom handlers and filters are outside this surface.

## Synchronous subprocesses

`subprocess.getstatusoutput(cmd, *, encoding=None, errors=None)` runs the command
in the native shell and returns its signed exit status and combined stdout and
stderr. `getoutput` returns the same text alone. Both normalize CRLF and CR to
LF and remove one final newline. The default decoder is UTF-8; UTF-8 with BOM,
ASCII, and Latin-1 use the same codecs as native path text reads.

On POSIX, `shutil.which(cmd, mode=os.F_OK | os.X_OK, path=None)` searches for a non-directory accessible with the requested mode. It returns an owned string, bytes, or `None`. String and native `Path` commands produce string paths, and bytes commands preserve arbitrary filename bytes. The search path accepts strings, bytes, native paths, or `None`. A command containing a directory component is checked directly. A missing `PATH` uses the platform default, an empty search path finds nothing, and empty entries in a nonempty colon-separated path search the current directory. Native string results must be representable as UTF-8. `os.F_OK`, `os.R_OK`, `os.W_OK` and `os.X_OK` are available for mode expressions.

The runtime supports a sealed `subprocess.run` boundary with exact `list[str]` arguments. Output defaults to the inherited descriptors. `capture_output=True` captures both streams. Each `stdout` or `stderr` may also select `subprocess.PIPE`, `subprocess.DEVNULL`, `sys.stdout`, or `sys.stderr`. `stderr=subprocess.STDOUT` follows the child's stdout, including when it is a pipe. Captures return bytes by default. Literal `text=True` or UTF-8 `encoding` decodes captured bytes strictly and normalizes newlines. Inherited, discarded, and redirected fields are `None`. `check` is a boolean expression, defaulting to false. `cwd` accepts a string, native `Path`, or a nullable form of either, and `env` accepts `dict[str, str]` or `None`. Arguments may be positional or named `args`; the other parameters are keyword-only. All supplied expressions evaluate once in source order before launching the child, even when keyword order differs from parameter order.

```python
import os
import subprocess
from pathlib import Path

environment = os.environ.copy()
environment["REPORT_MODE"] = "json"
result = subprocess.run(
    ["/usr/bin/printf", "%s\\n", "hello"],
    capture_output=True,
    encoding="utf-8",
    check=True,
    cwd=Path("."),
    env=environment,
    timeout=15,
)
print(result.stdout)
```

The call waits for the child and inherits stdin. It closes other inherited descriptors in the child and restores the default pipe and file-size signal dispositions. Executable lookup respects the supplied environment and child working directory. Arguments pass literally to the executable; shell expansion is never implicit. Text captures translate CRLF and bare CR to LF. Byte captures preserve the original stream. Decoding happens after the child has been reaped and before a nonzero status is checked.

The result exposes read-only `args: list[str]`, `returncode: int`, and the two stream fields. Each stream has the exact type `str`, `bytes` or `None` selected by that call. `CompletedProcess[str]` and `CompletedProcess[bytes]` annotate results that capture both streams. A bare annotation retains the existing two-text-captures type. Signal termination produces a negative return code. Results can be stored in classes, containers, and optionals. Equality compares identity; `str`, `repr`, formatted output, and `isinstance` support the sealed result. `args` retains the original argument list, so later list mutation remains visible through that field. Calling `check_returncode()` immediately checks the stored status; escaping that bound method is unsupported.

`check=True` or a failing `check_returncode()` raises the catch-only `subprocess.CalledProcessError`, which matches `subprocess.SubprocessError` and `Exception`. It exposes read-only `returncode: int`, `cmd: list[str]`, and the captured or absent `stdout`, `output`, and `stderr` fields. `stdout` and `output` expose the same capture. `cmd` aliases the original argument list, and the diagnostic reflects its current contents. These fixed getters raise `AttributeError` on unrelated exception instances. Missing executables and invalid working directories retain their ordinary operating-system error categories; they are not subprocess status errors.

On POSIX, `os.open(path, flags, mode=0o777, *, dir_fd=None)` accepts text, bytes, or a native `Path`. It and `os.dup` and `os.pipe` return non-inheritable descriptors. `os.dup2` accepts an optional `inheritable` flag (true by default). Use `os.close`, `os.isatty`, `os.get_inheritable` / `os.set_inheritable`, and `os.get_blocking` / `os.set_blocking` directly on integer descriptors. `os.lseek(fd, position, whence)` reads or moves the shared offset. The runtime supplies the platform's open flags and `SEEK_SET`, `SEEK_CUR`, `SEEK_END`, `SEEK_DATA`, and `SEEK_HOLE` constants.

On POSIX, `select.poll()` creates an independent poller. Its positional methods are
`register(fd[, mask])`, `modify(fd, mask)`, `unregister(fd)`, and `poll([timeout])`.
Descriptors are integers; the poller never owns or closes them. A repeated registration replaces the mask.
Timeouts are milliseconds: zero checks immediately, and a negative number or `None` waits indefinitely.
Results are fresh `(fd, mask)` tuples. The kernel can report `POLLERR`, `POLLHUP`, or `POLLNVAL`
without those flags being requested. Unregister a descriptor before closing or reusing its number.
An event loop can register several clients for `POLLIN` and enable `POLLOUT` only while bytes remain to send.

`select.select(read, write, exceptional[, timeout])` accepts three integer lists and an optional timeout
in seconds. It returns three fresh lists in input order, including duplicate ready descriptors.
Closed descriptors raise `OSError(EBADF)`; descriptors outside the target's `FD_SETSIZE` raise `ValueError`.
The native poller is not subject to the select fd-set limit. Objects with a dynamic `fileno()` and Python signal
callbacks are outside this integer-fd interface.

`selectors.DefaultSelector` uses `selectors.PollSelector`, the most capable backend
exposed by this build. Both support integer descriptors, `register`, `modify`,
`unregister`, `select`, `get_key`, `close`, and `with`. The key is an immutable
`SelectorKey(fileobj, fd, events, data)` namedtuple. Ready results pair that same
key with `EVENT_READ` and/or `EVENT_WRITE` restricted to its registered interests.
Timeouts are seconds; a nonpositive timeout checks immediately. Opaque data can
use the compiler's boxed values (`None`, booleans, integers, strings, bytes and
supported tuples). `get_map`, arbitrary data objects and automatic `fileno()`
conversion are outside this selector subset. Pass a socket's integer `fileno()`.

`os.environ.get(key)` and `os.getenv(key)` read a string from the environment and return `None` for a missing key. Both accept an optional `default` of type `str | None`. A string default gives a `str` result. `os.getcwd()` returns the current directory, and `os.fspath(path)` accepts a `str`, `bytes`, or native `Path`. `print(text, file=sys.stderr)` writes a string and a newline to standard error.

`shlex.split(s, comments=False, posix=True)` accepts a string or `None`
and boolean mode flags. `shlex.quote(s)` quotes a POSIX shell word;
`shlex.join(split_command)` quotes each item from a supported iterable before
requesting the next. Both handle false scalar values and concrete containers
before checking their type, as CPython does. The stateful `shlex.shlex` lexer
is unsupported.

A discarded `subprocess.run(argv, check=True,
stdout=sys.stderr)` forwards both child streams to the parent’s stderr while
the child is running. Failure raises `CalledProcessError` with uncaptured
streams set to `None`. A handler around direct calls with the same stream mode,
including `check_returncode()`, sees each field's exact `str`, `bytes`, or
`None` type. If a user call, explicit raise or different mode can supply the
exception, the field type includes the representations used by the program.

For programs that explicitly launch Python helpers, `sys.executable` is the
absolute interpreter path selected by `IMRE_PYTHON`, or the first executable
`python3` on `PATH`. It is an empty string if neither is available. The
native program itself has no Python runtime dependency. `sysconfig.get_path("scripts")`
locates the console scripts directory for that interpreter on standard POSIX
installations and in virtual environments. `os.path.exists(path)` checks
whether the supplied path exists. `__file__` refers to
the module’s source location at build time, so neighboring resources must
remain available when the program accesses them.

`sys.getdefaultencoding()` and `sys.getfilesystemencoding()` return the native text encoding (`"utf-8"`). The native standard streams expose that encoding as `.encoding`, including through `sys.__stdin__`, `sys.__stdout__` and `sys.__stderr__`.

`sys.platform` gives the target platform name; `os.name` gives its OS interface (`posix` or `nt`). `sys.stdout.write(text)` and `sys.stderr.write(text)` return the number of Unicode characters written. Each stream’s `flush()` flushes that stream.

`_opcode.ENABLE_SPECIALIZATION` and `_opcode.ENABLE_SPECIALIZATION_FT` are both the integer `0`: Imre emits native code and does not specialize CPython bytecode at runtime. Other `_opcode` operations are unsupported.

`sys.exception()` returns the actual exception being handled, or `None` outside a handler. Nested handlers restore their caller’s active exception. Exception identity can be compared with `is` and `is not`.

`sys._jit.is_available()`, `is_enabled()` and `is_active()` all return `False`:
Imre compiles before launch and cannot switch a running frame into a JIT.
`PYTHON_JIT` therefore does not change these predicates.

`sys.exit()` raises `SystemExit` through the ordinary exception and `finally` paths. It accepts an optional `None`, boolean, integer or string. An uncaught string prints to stderr and exits with status 1. An omitted or `None` code exits with status 0. A caught `SystemExit.code` preserves the scalar for printing, truth tests, equality, and type tests.

`os.environ.copy()` returns an independent `dict[str, str]` snapshot. Passing a dictionary as `env` replaces the child's environment; use a snapshot to preserve inherited variables before making overrides. `env=None` inherits the existing environment. Snapshotting undecodable Unix names or values raises `UnicodeDecodeError`; surrogateescaped strings remain outside Imre's string domain. NUL-containing arguments, directories, or environment entries, and environment keys containing `=`, raise `ValueError`. Imre accepts an empty environment name through `execve`, as does CPython's fork/exec path. CPython's `posix_spawn` path raises `ValueError` for that name.

The optional `timeout` keyword accepts signed integer literals or expressions of exact type `float | None` in seconds. `None` waits without a deadline. A deadline expiry kills and reaps the direct child, closes the capture pipes, and raises catch-only `subprocess.TimeoutExpired`, which matches `SubprocessError` and `Exception`. Its read-only fields include `cmd: list[str]` and `stdout`, `output`, and `stderr: bytes | None`. The exception retains the original integer or float for its message and `.timeout` field. A program can read `.timeout` when all its deadline sites agree on a numeric type. Partial captures remain bytes even in text mode, so incomplete UTF-8 is preserved. During pipe capture, an observed empty stream is `b""`; a stream not observed before the deadline is `None`. If both pipes close and waiting for the child itself expires, all capture fields are `None`, even after earlier reads, matching CPython. The runtime does not wait indefinitely for descendants holding inherited pipes. This API terminates the direct child and does not manage a process group.

An `except subprocess.TimeoutExpired as error` binding has these exact field types. Stored generic exception values can be narrowed with `isinstance(error, subprocess.TimeoutExpired)` before accessing timeout fields. `cmd` retains the original list and `stdout` aliases `output`. Launch precedes timeout validation, so a missing executable takes precedence over an invalid timeout. Zero and negative timeouts, including negative infinity, launch the child and expire immediately; NaN raises `ValueError`, while positive infinity or an unrepresentable polling duration raises `OverflowError`. Exceptions can be retained and re-raised through the existing sealed exception storage; constructing or annotating a `TimeoutExpired` value directly is unsupported.

Other subprocess modes are rejected, including `shell`, `Popen`, input data, arbitrary stream redirection, descriptor options, arbitrary encodings, and custom path or mapping protocols. Result construction, field replacement, exception construction, inheritance, and general access or mutation through `os.environ` are unsupported. For asynchronous child processes, see [native asyncio subprocesses](#native-asyncio-subprocesses).

## Structural pattern matching

`match` evaluates its subject once and tries cases in source order. Patterns support scalar literals, the `None`/`True`/`False` singletons, captures, `_`, `|`, `as`, fixed tuple and list sequences with one starred rest, dictionary mappings with optional `**rest`, and keyword fields on declared classes. Guards use Python truth tests and run only after a successful pattern. The first successful guard selects one case; cases do not fall through.

```python
def describe(rows: dict[str, list[int]]) -> str:
    match rows:
        case {"counts": [first, *tail], **other} if first > 0:
            return f"first:{first}, tail:{len(tail)}, other:{len(other)}"
        case {}:
            return "no positive first count"
        case _:
            return "unreachable for a dictionary"
```

Captures own their values before the guard runs. A false or raising guard leaves those bindings in place. Subsequent cases inspect the original captured subject, including mutations to that container made by an earlier guard. Starred sequence rests are new shallow lists, and mapping rests are new shallow dictionaries retaining the original insertion order. Contained lists and class instances keep their identities. Hidden subject and pattern roots are released on normal exit, exceptions, returns, and loop transfers.

Imre commits captures only after the complete pattern succeeds, so a failed pattern leaves existing bindings unchanged. Python deliberately leaves partial bindings from failed patterns unspecified; programs intended to run under both implementations must not observe them. This does not apply to failed guards, whose successful pattern bindings are retained in both implementations.

Each capture has one concrete storage type, following ordinary assignment rules. OR alternatives must bind the same names with the same type, or types that join as `T | None`. Duplicate captures, unreachable alternatives after an irrefutable pattern, and an unguarded irrefutable case before the final case are rejected. A local used after a match must be assigned on every path reaching that use. A successful non-None pattern narrows an optional subject; a failed `None` pattern permits later cases to use its payload. Only a still-unmodified local subject receives those facts.

A match on `tuple[bool, bool]` is exhaustive when unguarded literal pairs cover all four combinations. OR patterns may group pairs. For `match (left, right)` on two locals, a final wildcard can contain `assert_never((left, right))` as its sole statement once the four pairs have been covered. The assertion must repeat both locals in order, and a capture or guard that rebinds either local invalidates the proof.

Sequence patterns require concrete lists or fixed tuples, including an optional subject after excluding `None`. Tuple patterns cannot request more fixed positions than the declared tuple contains, and a starred tuple rest must fit one concrete list element type; `T` and `None` can form `list[T | None]`. An empty tuple rest uses an existing capture's list annotation when available and otherwise defaults to `list[int]`. Strings and user-defined sequence protocols are excluded. Mapping patterns require a concrete dictionary and literal keys. A missing key fails the pattern even when the value type is optional; a present key containing `None` remains distinguishable. Extra entries are permitted, and `{}` matches any dictionary.

Numeric literal patterns use exact Python value equality across integers, floats, and booleans, so `case 1.0` matches both `1` and `True` without changing the captured value's type. Huge integers are never rounded to floats for comparison. Singleton patterns use identity, so `case True` does not match `1` or `1.0`. Bytes literals compare bytes exactly and do not match strings.

Mapping literal keys use the dictionary's scalar equality, including `True` and `1.0` matching an integer key `1`. Finite integral float keys are normalized to their exact integer value; fractional and infinite float keys cannot match integer dictionaries and fail the pattern without a conversion error. Ordinary dictionary writes still require the exact declared key type. Repeated equal literal keys are rejected, including numeric aliases, signed zero, bytes, and repeated fractional or infinite floats, even for a dictionary with a different key domain.

Class patterns resolve imported aliases to a fixed nominal identity and inspect declared fields in keyword order. Another declared class does not match even if its fields have the same names. Positional class patterns, `__match_args__`, builtin class patterns, undeclared fields, dynamic value-pattern lookups, inheritance, metaclasses, and property lookups in patterns remain unsupported. All case bodies and guards are checked, including unreachable cases.

## String enums and literal strings

`enum.StrEnum` declares a closed set of string-valued members. Use a string literal, a previous member, or `auto()` on an ASCII member name. Imports and re-exports preserve the class identity. The first member with a given value supplies its canonical `.name`. Imre stores a member as an integer tag and shares the strings behind `.name` and `.value`.

```python
from enum import StrEnum, auto
from typing import Literal

class Source(StrEnum):
    OLD = auto()
    PREVIOUS = OLD
    NEW = auto()

def heading(source: Source, *, verb: Literal["added", "removed"]) -> str:
    return f"{source.value}: {verb}"

print(Source.PREVIOUS is Source.OLD)      # True
print(heading(Source.NEW, verb="added"))  # new: added
```

`==` compares string values, including across enum classes. `is` compares the class and canonical member. `str(member)` returns its value and `repr(member)` includes the class, canonical name, and quoted value. Sorting and typed keys use the string value. A qualified member in a `match` pattern uses value equality. Covering all canonical members with unguarded cases satisfies a function’s return check. For a nullable subject, cover `None` as well or narrow it first. Enum methods use the ordinary typed method rules. A user-defined function or field declared as plain `str` requires an explicit `member.value` or `str(member)`. Dynamic enum construction and custom enum hooks are outside the supported subset.

Use `typing.assert_never(member)` as a statement to check that every alternative of a local string enum has been handled. A nullable enum also requires handling `None`. Imre asks ty for the argument’s flow type at that line and accepts the call when it is `Never`. Adding a member without handling it makes the program fail to compile, including when the call carries a type-ignore comment. Calls inside a `finally` suite are rejected because that suite can also run during a return, break, or exception. For a member declared with `auto()` and then aliased, ty can require guards for both member names to prove exhaustion.

`Literal["added", "removed"]` accepts exactly the listed strings in parameters, returns, defaults, fields, and typed containers. A smaller set of allowed values can flow to a larger one or to `str`. A direct equality check against a fixed string can narrow a plain string local. Reassigning the local discards that proof. These annotations retain ordinary string storage and operations. Mutable containers keep their declared element types to protect writes through aliases.

## Fixed classes

An instance of a sealed `@dataclass(slots=True)` can be passed to an
`object` parameter or returned as `object` when the class has generated
initialization, equality and repr, no user-defined special methods or special
fields, and only scalar or fixed-tuple fields. The object keeps the original
instance alive: mutation through another reference is visible. Equality,
identity with a typed instance, truth, repr, and `isinstance` consult its
class. Guard a local with `isinstance(value, MyDataclass)` to read its fields.
A dataclass has neither tuple length nor indexing; both operations raise its
ordinary Python `TypeError`. Inheritance, mutable container fields, and
unguarded access on a general `object` are outside this subset.

Class bodies may bind a string literal or a fixed tuple of up to 1024 string
literals. A tuple may alias an earlier tuple in the same class or concatenate
it with a literal suffix. Reads through the class, an instance, and an
inheriting class share the stored value. These attributes cannot be reassigned
or shadowed by an instance field or a subclass.

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

Ordinary class bodies contain field annotations without defaults, typed methods, and optional docstrings. An instance method's first positional parameter is named `self`; its exact class type is inferred, or an explicit annotation must name that class. Other parameter and return types follow the same inference rules as direct functions; an initializer must return `None`. Method calls use the receiver's exact class and a fixed implementation. Passing or returning an instance preserves its identity, and aliases observe field replacement. Ordinary classes use identity for `==` and `!=`; distinct nominal classes compare unequal.

An ordinary class requires `__init__(...) -> None`, including a class with no fields. Its body may make straight-line local and field assignments, check assertions, and initialize inherited fields with `super().__init__`. Every declared field must be assigned exactly once. Expressions may use constructor arguments, locals and already initialized fields; the receiver itself cannot escape or be rebound. Conditionals, loops, explicit returns, and direct later calls to `__init__` are rejected.

When a checked guard narrows an optional record stored in a mutable field, each subsequent attribute read checks the current field; clearing it through an alias produces the usual `AttributeError`.

`@property` marks a getter with only `self`. Each read calls it again. A following `@name.setter` or a class binding `name = property(get_name, set_name)` uses a fixed instance method to handle writes. Assignment evaluates the value before the receiver and discards the setter’s return. Augmented assignment evaluates the receiver once, calls the getter before evaluating the right operand, then invokes the setter with the result. A subclass may replace a property, including by removing its setter. Deletion is unsupported. An ordinary fixed class can use `@functools.cached_property` on a getter with an annotated result: the first successful read caches the value on that instance, including `None`. A failed getter runs again on the next read. Overriding or deleting the cached attribute is unsupported. `@classmethod` binds a first positional parameter named `cls` to the declaring class; `cls(...)` and `cls.method(...)` resolve statically. `@staticmethod` has no implicit parameter. Class/static methods work through either a class or an instance, and an instance expression still evaluates before arguments. `Class.method(instance, ...)` also supports unbound instance methods. `typing.Self`, including aliases and quoted annotations, means the owning class in method annotations; it is not a runtime type object or a stored class value.

`@dataclass` and `@dataclass(...)` generate structural initialization, equality, and repr using the same fixed layout. Import aliases and reexports of `dataclasses.dataclass` and `dataclasses.field` retain their identity. This is a compile-time facility, not general decorator execution.

```python
from dataclasses import dataclass, field
from typing import Self

@dataclass(slots=True, frozen=True, kw_only=True)
class Snapshot:
    name: str
    values: list[int] = field(default_factory=list)

    @classmethod
    def named(cls, name: str) -> Self:
        return cls(name=name)

    @property
    def count(self) -> int:
        return len(self.values)

snapshot = Snapshot.named("sample")
snapshot.values.append(1)
print(snapshot.count)
```

Dataclass fields may use exact scalar literal defaults or `field(default_factory=list)`, `dict`, or `set`. Each omitted factory field gets a fresh container; explicit arguments retain their identity. Optional container factories preserve the distinction between an omitted value and explicit `None`. `kw_only` works at class and field level, including a field override. Fields also accept `compare` and `repr` flags. Flags require literal booleans; custom factories and mutable literal defaults are rejected.

Class options `init`, `eq`, `repr`, `frozen`, `kw_only`, and `slots` are supported. A mutable dataclass may retain an explicit initializer under the ordinary constructor rules; `init=False` requires one. Both values of `slots` keep the language's fixed-layout contract. Frozen records reject field assignments at compile time; their referenced containers remain mutable. Dataclasses accept exact string lists and fixed tuples of strings for `Iterable[str]` fields. They preserve the original container for representation, identity, truth, iteration, and length. The generated initializer calls an instance method `__post_init__(self) -> None` after assigning every field and default factory. An explicit initializer controls its own calls. Explicit frozen initializers, `InitVar`, `ClassVar`, `field(init=False)`, custom metadata, generated ordering, custom hashes, and weak-reference slots are unsupported. `match_args` is accepted without adding positional class patterns or runtime `__match_args__` access. Frozen records do not add class-valued dictionary keys or set members.

Generated equality follows modern CPython: an object equals itself; otherwise instances of the same dataclass compare selected fields in declaration order. `eq=False` preserves identity equality. Distinct records containing the same scalar NaN compare unequal. Compared container fields containing floats are rejected because native unboxed floats cannot preserve Python container NaN identity; use `field(compare=False)` or `@dataclass(eq=False)` for such storage. Nested record fields preserve their own identity and equality behavior.

Fixed classes and dataclasses can define `__str__(self) -> str`. `str(record)`, `print(record)`, `f"{record}"`, and `f"{record!s}"` call this method once. Without an override they use the record’s repr. The method also supports ordinary instance calls and bound method values.

Generated dataclass repr includes field names and values in declaration order, omitting `field(repr=False)`. `repr(record)`, `f"{record!r}"`, and representations of records inside containers use this repr even when the record defines `__str__`. With `repr=False`, the class uses the ordinary object repr, which includes its address. Resource fields require `repr=False` on the field or class.

Fixed classes reserve field names beginning with `__`. A fixed class can extend one concrete base and inherit its fields and methods. Its instances keep a statically known field layout. The compiler rejects multiple concrete bases, metaclasses and dynamically added fields. Class-field ownership types must form an acyclic graph, following list elements, dictionary values, tuple positions, and optional payloads. For example, fields `children: list["Node"]`, `named: dict[str, "Node"]`, and `parent: "Node | None"` inside class `Node` are rejected even when unused or initialized empty. Methods may accept or return their own class because those signatures do not create stored references.

Plain fixed classes may initialize unannotated scalar, list, set or dictionary attributes in their bodies. A class executes these assignments in order, once, at its definition. An initializer can call a typed helper and refer to an earlier assignment in the same class; an unqualified name does not search base classes. Instances inherit the same value, so mutating a shared container affects every unshadowed reader. Assigning to an instance creates an override; deleting it reveals the current class value. Assigning through a subclass name creates a subclass binding. Assignments retain the inferred data type. Literal strings and fixed tuples of strings keep their existing read-only class binding rules. These rules cover ordinary data attributes; arbitrary descriptors, class-body control flow, and deletion of class bindings are unsupported.

### Immutable value structs

Use the first-party decorator for a small immutable value whose identity is never observed:

```python
from imre import struct

@struct
class Point:
    x: float
    y: float

    def norm2(self) -> float:
        return self.x * self.x + self.y * self.y

points: list[Point] = [Point(1.0, 2.0)]
saved = points[0]
points[0] = Point(3.0, 4.0)
print(saved.norm2())  # 5.0
```

A struct is final and has one or more required, exactly typed `float` or `bool` fields.
The generated constructor accepts positional and named arguments. Fields are read-only;
update a list entry by replacing the whole value. Ordinary instance methods are allowed.
There are no field defaults, inheritance, or user-defined construction, equality, or hashing.

Struct equality compares fields of the same declared type. All float NaNs compare equal
*as fields of these values*, and the two signed zeros compare equal. Reading the float
itself still uses Python floating-point behavior. Hashing, list membership, dictionary
keys and set membership agree with that equality. Structs do not expose instance identity:
use `==`, never `is`, `id()` or a weak reference. `p is None` on an optional value
is supported. The enclosing list still has normal identity and mutation semantics.

Imre normally stores the scalar fields of consecutive elements of `list[Point]` contiguously.
Each field occupies an eight-byte slot; the example uses 16 bytes per element.
There is no record header, pointer, or reference count per element in that buffer.
For `points = [p] * n`, Imre can store one immutable point and the logical length.
Assigning `points[i] = q` changes that slot alone. Imre stores only slots whose
field bits differ from `p`; writing the original bits back removes the exception.
This includes the sign of zero, even though the two zeros compare equal. Copies,
slices, and pops can preserve sparse storage. When sufficiently many slots differ,
one write can reserve and fill a packed buffer in O(n); subsequent writes to it
update individual rows. These storage choices are private to Imre.
A point taken out of the list remains valid after replacement, growth or clearing.
The compiler can project fields directly or copy them to a stack temporary; uses that
need an independently owned record can allocate one. This layout is private to Imre.
On CPython, putting the repository's `stdlib/` on `PYTHONPATH` supplies the same
decorator and value operations, with standard slotted objects and a regular list.
CPython can still observe object identity; programs that do so are outside the contract.

## Concrete containers and optional values

An empty tuple or a fixed tuple of homogeneous `str`, `bytes`, `int`, or `bool` supports `in` and `not in`. The needle and all tuple elements evaluate in Python order before the first comparison.

`collections.deque[T]` stores one concrete element type in a native ring. Its
constructor accepts an iterable and an optional nonnegative `maxlen`. It supports
append, pop, and extend at either end, clear, shallow copy, equality, length,
truth testing, and forward or reverse iteration. In-place reversal preserves
the bound and the positions of active iterators. A saved append, pop, or reverse
method keeps its receiver alive. An unannotated empty deque defaults to integer elements.
Calling `d.__init__(iterable, maxlen)` validates the new bound, clears the
same deque, and consumes the iterable. If iteration raises, items already
appended remain visible through aliases. Omitting `maxlen` removes the old bound.
Integer and boolean indices support reads, replacement, deletion and numeric
augmented assignment, including negative positions. Replacement keeps an
iterator valid; deletion invalidates it. Membership supports homogeneous
integers, strings and booleans.

`list[T]` has one concrete element type, such as `list[str]`, `list[Counter]`, or `list[list[int]]`. Types are invariant: an existing `list[str]` cannot become `list[str | None]` through assignment or a call. Every append and indexed write checks the exact element type. Repetition preserves element values and shares reference-type elements across positions. Lists of immutable value structs store scalar payloads inline; `list[int]` uses specialized tagged-integer storage. Other lists use owned runtime slots. Calling `reverse()` reverses either list in place and returns `None`; aliases and active iterators see the new order.

An unquoted `Sequence[T]` parameter from `collections.abc` or `typing` accepts
an exact `list[T]` or an omitted `()` default. Its body may iterate the
parameter, apply `map(str, parameter)`, or call `len`. Parameters without a
tuple default also support read-only indexing and slicing.
It shares the caller’s list, so mutations through existing aliases remain visible.
A function returning `Sequence[T]` can return an exact `list[T]`.

`dict[K, V]` accepts exact `int`, `str`, `bytes`, `Path`, or immutable fixed-tuple keys and one concrete value type. Tuple keys may recursively contain `int`, `bool`, `str`, `bytes`, `Path`, `None`, and nullable forms of those immutable fields. Floats, records, mutable containers, and the `str | bytes` union are excluded from keys. Keys use value equality under one exact type; booleans cannot stand in for integer keys, and a bare `bool` is not a key type. Nonempty literals infer these types from their first entry; `{}` and `dict()` require a concrete contextual type. Duplicate keys keep the first insertion position and the last value. Replacing a value keeps its position, while deletion and reinsertion move the key to the end. `mapping[key]`, assignment, `del mapping[key]`, integer augmented assignment, `len`, and single `in`/`not in` comparisons are supported. Missing required reads and deletes report `KeyError`.

`mapping.get(key)` returns `V | None`, flattened when `V` is already optional or `None`. A default must fit `V | None`; a guaranteed `V` default gives a `V` result. `mapping.pop(key)` removes and returns a stored `V`, raising `KeyError` on a miss. With a default, it follows the same result typing as `get`. Defaults are evaluated eagerly even when the key exists. Container literals in defaults receive the dictionary's declared value type. Dictionaries are invariant in their key and value types.

`collections.Counter[K]` uses exact string or integer keys and arbitrary-precision integer counts. `Counter(iterable)` counts each key, and `Counter(mapping)` copies integer counts. A bare `Counter` annotation or unconstrained empty `Counter()` uses string keys. `Counter[int]()` and an annotated `Counter[int]` destination select integer keys. String-key counters also accept keyword counts. `update` adds its positional iterable or mapping and then the keyword counts, retaining negative and zero entries. A missing subscription returns zero without inserting. Membership, `get`, key iteration and `items()` inspect the stored entries. Deleting an absent key is a no-op. `total()` and `sum(c.values())` sum all counts. Frozen dataclasses can hold independently created counters with `field(default_factory=Counter)`. `subtract(iterable, **counts)` decrements instead of replacing counts and retains nonpositive values. Typed mappings contribute their counts; other iterables contribute one per occurrence. Bound and unbound update/subtract calls support native argument errors.

Counter equality and multiset inclusion (`<`, `<=`, `>`, `>=`) compare the counts for every key of either counter. An absent key contributes zero.

`Counter.elements()` returns an iterator over the stored keys in insertion
order. Each positive integer count repeats its key; a change to a later entry
is visible when the iterator reaches it.

`_collections._count_elements(mapping, iterable)`, also exported by
`collections`, adds one per yielded key. It accepts dictionaries with integer
counts and concrete classes with `get` and `__setitem__` methods. It updates
the receiver before requesting another key, so mutations survive later errors.

`mapping.clear()` removes every entry in place and returns `None`. Aliases see
the empty dictionary; a `Counter` still returns zero for a missing key, and a
`defaultdict` keeps its factory. `dict.clear(mapping)` and the inherited class
methods `Counter.clear(mapping)` and `defaultdict.clear(mapping)` do the same.

`Counter.copy()` preserves its zero for missing keys, and `defaultdict.copy()`
preserves the original factory. Each copy holds the original keys and values.

`mapping.update(other, key=value)`
evaluates its arguments first, replaces existing values without moving their
keys, then appends new keys in order. The positional source can be any supported
dictionary of the same key and value types; keyword entries require string
keys. A Counter retains its additive update behavior through a dict annotation.

`dict(source, key=value)` constructs a plain dictionary from a typed mapping
or an iterable of two-field tuples. Entries from the source precede the keyword
entries; replacing a key keeps its original position. The new dictionary shares
stored values with its input. A missing key raises `KeyError` even when the
source was a Counter or defaultdict.

Unary `+` and `-` select positive counts and negated negative counts. Binary `+`, `-`, `|` and `&` return a fresh Counter with positive counts, preserving the surviving left keys followed by new right keys in insertion order. Augmented assignment to a Counter local mutates the existing object, including when the right operand aliases it.

`mapping.copy()` on a plain typed dictionary returns a fresh dictionary in
insertion order. Each key and value is shared with the source; inserting or
deleting an entry in the copy leaves the source dictionary unchanged.

`array.array` is available as a class value: it retains its module, name,
representation and identity. `issubclass` recognizes its virtual registration
with `collections.abc.MutableSequence` and `collections.abc.Sequence`, and
distinguishes both from `collections.abc.MutableMapping`.

`dict.fromkeys(iterable, value)` builds a fresh dictionary in first-key order.
It evaluates the value once, defaults it to `None`, and shares that object
among all keys. A bound call uses the receiver's class without copying its
entries. A native defaultdict produces a new mapping with no default factory;
a native Counter raises `NotImplementedError`.

`collections.OrderedDict[K, V]` accepts the same typed mappings, pairs and
keyword entries as `dict`. Replacing a value preserves its position; deleting
and reinserting the key moves it to the end. Two ordered dictionaries compare
both their keys' order and their values. When one operand is a plain dictionary
of the same key and value types, equality ignores order. The bound `copy()`
retains the concrete mapping type and shares its values. Changing keys during
iteration raises `RuntimeError` if another key remained to be yielded.

`collections.defaultdict[K, list[T]]` accepts `defaultdict(list)` in a typed context. A missing subscription inserts a fresh list and returns it. Further subscriptions return that same list through every alias, including a `dict[K, list[T]]` parameter. Membership, `get`, and key unions leave missing keys absent.

Its `default_factory` can be read and set to `list` or `None` through a typed
defaultdict. An explicit `__missing__(key)` call runs the list factory, then
returns the stored object if the key already exists. With no factory it raises
`KeyError` for any key of the declared type, even one present in the mapping.

`collections.ChainMap[K, V]` accepts dictionaries with matching key and value
types and exposes them in its mutable `maps` list. A no-argument constructor
starts with a fresh dictionary. Replacing `maps` affects all aliases of the
ChainMap; callers that saved the previous list still hold that list. Truth
testing checks whether any underlying dictionary is nonempty. `repr` uses the
current mappings in order. Subscription searches the layers in order, including
the missing-key behavior of a Counter or defaultdict. Membership never inserts a
key. `get(key, default=None)` tests membership before subscribing, and indexed
assignment writes through to the first dictionary.

Direct `mapping.keys()` operands and sets of the same key type support `|`, including chained unions. Each union reads the two key sources after evaluating both operands, and returns an independent set. Stored key-view objects and arbitrary iterables as union operands are outside the supported subset.

`set[T]` accepts the same exact scalar or immutable fixed-tuple shapes as dictionary keys, with the same value equality. Nonempty literals infer their element type and evaluate every element from left to right, including duplicates. Empty `set()` requires a concrete contextual type. `add(value)` inserts an element, `discard(value)` removes it when present, and both return `None`. Adding a duplicate or discarding a missing element is a no-op. `len`, single `in`/`not in` comparisons, direct iteration, and loop-head `enumerate` are supported. Set iteration and display order are unspecified. Iteration checks size changes and owns each yielded element before the body can remove it.

`set(cursor)` consumes a native iterator from its current position and owns each distinct element. `set` can also consume ranges, generator expressions and typed iterator pipelines directly. An empty `set()` in a conditional can take its element type from the other branch.

Sets with the same checked element type support equality, inequality and f-string display. Union (`|`), intersection (`&`) and difference (`-`) on sets or dictionary key views return a fresh set. Symmetric difference, subset/superset comparisons, `remove`, `pop`, and `clear` are unsupported.

After a terminating guard such as `if len(row) != 3: raise ValueError(...)`, `tuple(row)` can materialize a fixed tuple from a scalar list. A checked replacement of a list element preserves the length; a resize, rebinding, or unknown call invalidates it. Tuple types name each position, for example `tuple[str, int, list[Counter]]`; `tuple[()]` is empty. A tuple's slots cannot be reassigned, but a referenced list or instance can still be mutated. Indexes into a fixed positional layout must be in-range integer literals, optionally negative. Homogeneous runtime-sized tuples accept integer expressions and raise `IndexError` at runtime. Tuple concatenation and repetition are not supported. Tuple equality and inequality require the same concrete immutable shape accepted for dictionary keys. Ordered scalar tuple shapes additionally support lexicographic `<`, `<=`, `>`, and `>=`, including nested tuples and fields whose exact type is `None`. Nullable tuple fields permit equality but exclude ordering. Lists of matching immutable scalar tuples support `==` and `!=`. They compare lengths and then tuple values in order. Mutable tuple fields and float fields are outside this comparison.

Fixed tuples, lists, strings, and bytes support the nested and starred assignment patterns described in [assignment and container expansion](#assignment-and-container-expansion). Concrete list lengths are checked at runtime; fixed-tuple shapes are checked during compilation.

A plain `for` loop can iterate a fixed tuple with one element type, including a module constant. The tuple is evaluated once before the body runs.

`for value in values` evaluates a concrete list once and retains that original list even if the source name or field is later replaced. It reads the current length and acquires the next element at each step. Appending, removing, or replacing elements through an alias therefore has Python's index-based list-iteration behavior. The current item remains owned if the body removes it from the list. `enumerate(values, start)` captures the list before evaluating its optional integer start and yields fixed `(int, T)` tuples; its counter supports arbitrary precision and is independent of visible target rebinding. Both forms support `break`, `continue`, nested loops, and early returns. A target first introduced by a loop remains potentially uninitialized afterward because the loop may be empty. Escaping iterator values and user-defined iteration protocols are not supported.

Immediate consumers also accept `iter(lambda: expr, sentinel)` for an exact `str` or `bytes` result and a sentinel of the same type. The sentinel is evaluated once. Each pull reads the current enclosing locals; a matching result or a producer raising `StopIteration` ends iteration. Other errors propagate. The nullary lambda does not escape. Imre rejects a delayed pull that reads an exception binding the loop body can delete.

`for` and `while` support an `else` suite. It runs when iteration is exhausted or the condition becomes false, including zero iterations. `break` skips it; `continue` does not. Returns and escaping errors skip it. A `finally` block can override any pending transfer: a break replaced by continue can eventually reach else, while a continue replaced by break skips it. The completed loop is no longer an enclosing loop for its else suite, so `break` and `continue` there refer to an outer loop. Compiler-owned iterator and source roots are released before the else suite, preserving drop-time file flushing; the visible loop target keeps its usual last-item binding.

The checker joins assignments from surviving break paths with the normally completed else suite. A search can assign its result before every break and assign a fallback in else, then use the result afterward. An else suite does not make the iteration target initialized when the iterable was empty. False-condition optional facts are available inside a while-else suite; finally writes and exception-handler target deletion cannot preserve stale facts on break paths.

`for key in mapping` and `for key, value in mapping.items()` visit insertion order. `enumerate` accepts either form and creates the iterator after evaluating its start argument. `for value in mapping.values()` visits values in the same order. Each yielded key or value has its own owner before the body runs. If the loop replaces an existing value, later iterations see the replacement. A changed size at the next step reports `RuntimeError`, including after the last yielded item. Same-size structural mutation may report changed keys or skip entries. Mapping views are supported where the loop or reduction immediately consumes them.

Literal elements receive types from an annotated variable, an existing storage location, a function or method argument, or a return annotation. This also applies inside nested literals and list repetition: `rows: list[list[str]] = [[], []]` creates two independent string lists. It does not convert an existing container to a different element type.

`T | None` permits a value of exactly `T` or `None`. Storage, call, and return boundaries enforce the optional's exact payload type; a bare return or falling off an optional-returning function produces `None`. The bounded `str | bytes` union described above is also supported; arbitrary unions are not. Use `value is None` or `value is not None` before accessing an unknown optional payload. Direct checks of local names narrow the relevant branches, short-circuit operands, assertions, and guarded loop bodies; early exits preserve facts on the remaining path. Rebinding updates those facts, and possible stores in a loop invalidate facts that later iterations could contradict.

Narrowing applies to local bindings, not mutable fields or indexed elements. Copy a field or element into a local before checking it; the local owns its value even if an alias later replaces the original slot. Compare an optional payload only after narrowing it. A truth test also narrows a true optional local to its payload; a false optional container may still be an empty container.

## Eager comprehensions

List, dictionary, and set comprehensions support nested `for` clauses and zero or more filters using Python truth tests per clause. Sources include concrete containers, strings, bytes, ranges, and the supported immediate iterator pipelines. Targets use the same unpacking rules as ordinary loops.

```python
def positives(rows: list[list[int]]) -> list[int]:
    return [value for row in rows for value in row if value > 0]


names: list[str] = ["alpha", "beta", "alpha"]
unique = {name for name in names}
last_position = {name: index for index, name in enumerate(names)}
```

The leftmost iterable uses the enclosing scope and is evaluated once. All clause targets belong to the comprehension's separate scope, including names first assigned by later clauses; they cannot fall back to an outer binding before assignment. Targets may shadow an enclosing local of another type and never replace that outer binding. Nested comprehensions have their own scope. Static module/definition bindings and reserved builtins retain their existing shadowing restrictions.

Clauses nest from left to right. Each inner iterable is evaluated when its enclosing iteration reaches it; filters short-circuit in source order, and the element expression runs only after all filters pass. Dictionary keys are evaluated and owned before their values. Duplicate dictionary keys retain their first insertion position and last value. Set traversal remains unspecified.

Results have exact element/key/value types, inferred from the checked body or supplied by a typed storage, argument, or return boundary. Context also reaches nested literals and comprehensions. Filters can narrow optional local targets before constructing an element. These rules do not convert an existing container to a different element type. Filters use the same truth tests as ordinary conditions.

A final filter such as `if row.tag is not None` also narrows a direct read
of that stored field in the result. In a dictionary display the key must
preserve the field before the value reads it. Reading other stored fields or
`Path.name` satisfies that requirement. A call that can mutate the row does
not. Ordinary f-strings render a `str | None` field as its text or `"None"`.

Capturing an outer local preserves its value and shared object identity; field and indexed reads still occur where written inside the body. Mutations through aliases retain the existing list/dictionary/set iteration behavior. A failure propagates through normal handlers and finally blocks, releasing the partial result and temporary owners without rolling back earlier user-visible mutations. Eager comprehensions do not expose an iterator, closure, callback, or partially built result. Generator expressions use the separate lazy rules above; other generator functions, async clauses, and user-defined iteration protocols remain unsupported.

## Ownership and errors

Assignment and return expressions produce owned references when their type needs one. Operator and call operands may borrow a direct local only when later expression evaluation cannot replace that binding; earlier operands retain separate owners across assignment expressions. Parameters that can own a file take a reference on entry, so rebinding can close the previous value inside the function. Other unchanged parameters can borrow the caller's root. Owned local slots release their values on exit. A return acquires or transfers ownership even when returning a borrowed parameter. List-element reads generally remain owned across later evaluation; field and tuple reads, optional unwrapping, pops, and computed results are also owned, so mutation cannot invalidate an intermediate value. This also keeps a method receiver alive if later arguments replace its original field. An assignment publishes the replacement before dropping the old value. Ordinary subscript and field assignment evaluate the RHS first; augmented assignment instead captures the target and old value before evaluating its RHS.

Integer representation is canonical: values from `-2**62` through `2**62 - 1` always use tagged machine words; values outside that range use `num-bigint`. Checked arithmetic promotes and demotes as needed. Foreign adapters must preserve this invariant before passing integers to generated code.

Conservative interval facts track immutable integer values in local slots across reassignment and control flow. Heap reads and user-call results start unknown. Direct arithmetic requires proofs that both operands and the result fit the small representation; only proved representable arithmetic and tagging shifts receive LLVM's `nsw` overflow flag. Unknown bounds, analysis overflow, or a possibly zero divisor retain checked lowering. Division by zero remains a language error at every optimization level. `range` arguments are captured once, and a specialized hidden cursor must also fit after its final increment; assigning to the visible target does not change its progression.

A whole-function proof can omit old-value and exit ownership checks for an integer local when every possible write or move into that slot is proved small. Parameters and unknown replacements disqualify that whole-slot proof. A separate assignment-time proof can omit old-value cleanup after the RHS succeeds when its first operand reads that exact slot as a proved small integer and later evaluation cannot replace, clear, or move the slot. Exit and exceptional cleanup retain their own ownership checks.

A proved nonnegative small index may use a decoded native-index ABI. Ordinary reads and all writes still check the container's current bounds. A separately guarded canonical reduction can prove an exact source/cursor read in bounds before its unique cursor advance, while the source remains rooted and unchanged. Such reads use an infallible borrowed accessor; reads after the advance, in the loop's else suite, at another index, or from another source retain checks. Unknown calls, mutation, hidden writes, and early continue paths must satisfy the proof or keep the original checked loop. Negative or arbitrary-precision indices retain their normal normalization and error behavior.

Element reads otherwise acquire ownership before later evaluation can remove them. A direct integer-list read in a two-operand integer comparison can also borrow when both complete operand trees are proved nonmutating; the local keeps the list rooted until the comparison consumes the handle. Chained comparisons and potentially mutating evaluation retain owned reads. A same-list copy with simple, proved indices can combine its get and set, checking and acquiring the source before checking and replacing the destination.

Heap objects are nonmoving reference-counted allocations. Classes and fixed tuples use concrete fixed layouts. A tuple made from runtime-sized spreads owns its field layout. Homogeneous lists, sets and dictionaries store representation metadata once per container rather than in each element. Optionals occupy one nullable word: zero means absent, present integers and pointers keep their payload word, and optional booleans encode False/True as one/two. Present optional floats own a separate immutable binary64 allocation so every float bit pattern remains available; ordinary floats remain unboxed. These representations do not change the supported Python values or shared mutation behavior.

Writes acquire the replacement before releasing the displaced value, and reads acquire ownership before the receiver can be released, apart from the explicitly proved rooted integer borrows above. Fixed records start with zeroed fields and use a static destruction descriptor, so failed construction safely releases every initialized owner. Generated field accesses are direct; their receiver remains rooted and the checker proves complete initialization before a read. Container storage uses scoped `UnsafeCell` closures on one execution thread: mutable access is exclusive, operations cannot reenter through aliases, and no storage reference or resizable-buffer address may escape. Independently owned values and explicitly constrained borrowed integer handles may escape; displaced owners are dropped after access ends.

Dictionaries use a randomized hash index and insertion-ordered entries. Integer, string, and byte keys have specialized storage; sets own keys without dummy value slots or an ordering guarantee. Internal iterators root their container and prevent entry compaction until released, but keep no storage reference across a loop body. The acyclic heap type graph prevents reference cycles through classes and containers. There are no finalizers or user callbacks under storage access. Threaded or reentrant extension boundaries would need to preserve these rules. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Language errors propagate through explicit checked call results and can be caught. Each fallible runtime call records an owned pending exception and reports failure through either its result or a pending-state query; generated code checks before decoding or acquiring a result. Integer-list reads reserve raw zero for failure, while writes return a 32-bit success flag. Other calls retain the pending-state check. Proven fixed-field operations and functions whose complete bodies cannot raise omit redundant queries; their argument and receiver evaluations retain their own checks. Partial expression values, constructor fields, unpacking temporaries, and loop iterator roots are released when their scopes exit. Function propagation also releases owned locals. No Rust panic or native stack unwinding implements language exceptions. Uncaught errors print an exception category and message and return process status 1 after cleanup. Actual allocation failures and invalid internal ABI operations may still terminate the process. The compiler and runtime archive must be rebuilt together when this private ABI changes.

Exceptions contain an immutable category, message, optional explicit cause, and context-suppression flag. Process status errors additionally retain the original argument list, return code, and captured strings, and format their message from the current argument list. A cause can be attached only while raising a freshly constructed exception, and cause attributes cannot be written, so cause chains remain acyclic. Deep uniquely owned cause chains are released iteratively. There are no traceback, frame, or implicit context references. Exited function frames release their owners during propagation; implicit file closure can therefore happen before CPython releases locals retained by a traceback. A source-defined exception can inherit from one known exception class. A subclass with a docstring or `pass` uses the inherited constructor and matches its ancestor handlers. A failing `asyncio.TaskGroup` can raise an exception group. Full traceback objects, implicit exception context, attaching causes to existing instances and `except*` remain unsupported.

## Exceptions and cleanup

A caught `Exception` or `BaseException`, including `cm.exception` from
`unittest.TestCase.assertRaises`, exposes its original positional arguments in
`error.args`. The tuple can outlive the handler. Its mutable contents still
refer to the original objects. Read its length, index it, or format it with
`repr`. An integer subscript on a retained dictionary looks up that key,
and a missing key raises `KeyError`. A retained exception from a task group
keeps its class and its own `str` and `repr` methods. An identity comparison with a known tuple (including another
`error.args` getter) compares the underlying tuple. Assigning a new `.args`
tuple and identity comparisons between arbitrary erased scalar values are
outside the native subset.

The args preserve how an error was raised. For example, a failed
`subprocess.run(check=True)` has `(returncode, cmd)`; a failed
`CompletedProcess.check_returncode()` also includes stdout and stderr.
`SystemExit(1, 2).args` is the same tuple as its code, while
`SystemExit((1, 2)).args` contains that tuple as one argument.

An explicitly typed local assigned in a `try` body may be read after a handled exception. When a simple assignment did not complete on that path and no earlier value exists, the read raises `UnboundLocalError` (a `NameError`). A successfully assigned `None` remains an ordinary value.

`ImportError` and its subclass `ModuleNotFoundError` retain their positional
arguments in `.args`. The keyword-only `name`, `path`, and `name_from` attributes
default to `None`; `.msg` is the sole positional argument, or `None`. Arguments
can be `None`, booleans, integers, strings, bytes, or fixed tuples of these
values. On a constructed or narrowed import exception, these fields support
`str`, `repr`, equality, truth, and type tests. Strings, bytes, and tuples also
support length and integer indexing. Reinitializing an instance resets omitted
fields. A fresh keyword dictionary display can supply the three fields. A
repeated key inside one display uses its last value; supplying the same key in
two call arguments raises `TypeError` before initialization. Arbitrary objects,
unpacking a separately stored dictionary, direct field writes, and pickling
remain outside this subset.

User exception classes keep their nominal identity in both builtin and user
handlers. A marker class may inherit several exception bases with a consistent
Python method resolution order. A class that carries fields can use an ordinary
typed initializer or a dataclass initializer with `__post_init__`. Its constructor
must initialize its base exactly once. An explicit `Exception.__init__(self, ...)`
or `BaseException.__init__(self, ...)` keeps the positional arguments in `.args`,
including `None`, booleans, integers, strings, bytes, and fixed tuples of those
values. The `super().__init__` form accepts zero arguments, one string, or
multiple supported values. Constructor keyword arguments, typed fields, and custom `__str__` and
`__repr__` methods use the same object after a raise. A class attribute can name
such a constructor. A subclass can extend one inherited payload or inherit its
constructor unchanged. A base handler sees its original fields, argument tuple
and formatter; a sibling exception remains distinct. Two independent payload
layouts, dynamic constructor selection and reinitializing a payload are unsupported.

An unannotated variadic exception initializer can store named parameters in
fields and call `super().__init__` once, forwarding parameters or its positional
tail. A subclass can do the same with fixed or variadic parameters. Supported
values include None, bool, int, float, str, bytes, fixed tuples and typed lists
of these values. Lists preserve identity and mutations in stored fields and
in `.args`. A list whose elements contain `object` cannot be stored this way: a later mutation could create a reference cycle. Scalar literal defaults and keyword-only parameters are supported;
computed or mutable defaults, positional-only parameters, `**kwargs`, arbitrary
objects, and arbitrary code inside these unannotated initializers are unsupported.

The constructible builtin categories are `Exception`, `ArithmeticError`, `ZeroDivisionError`, `OverflowError`, `LookupError`, `IndexError`, `KeyError`, `ValueError`, `AssertionError`, `RuntimeError`, `StopIteration`, `EOFError`, `OSError`, `TypeError`, `AttributeError`, `MemoryError`, `SystemError`, `UnicodeError`, `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`, plus the imported `io.UnsupportedOperation`. A constructor takes zero arguments or one supported argument (a string, number, bytes, container, or `None`). Mutable arguments remain live through `.args` and formatting. `raise Category` constructs the zero-argument form; `raise value` requires an exception instance. The annotation `Exception` admits this sealed family and can be used in parameters, returns, fields, containers, and optional values. Specific subclass annotations are currently rejected rather than erasing their narrower contracts. `BaseException` also works as an annotation for this family. Both `BaseException` and `UnicodeDecodeError` are supported as handler categories; directly constructing them is outside the subset.

Handlers select the first matching fixed builtin category or nonempty tuple of categories, using the builtin hierarchy. A bare handler catches any supported exception. `except Category as name` binds an owned exception and deletes that name on every handler exit, even if it previously existed or was rebound inside the handler. Save it under another name to retain it. Bare `raise` uses the dynamically active exception, including from a called function or a `finally` block unwinding an exception; without an active exception it raises `RuntimeError`.

`raise ValueError("message") from cause` and `raise ValueError from cause` accept an exception, supported builtin exception class, nullable exception, or `None` as the cause. The raised exception evaluates before the cause. Readonly `error.__cause__` and `error.__suppress_context__` expose the resulting metadata; `from None` sets suppression with an absent cause. Ordinary re-raising preserves existing metadata. Uncaught explicit chains print their categories and messages in cause order, without traceback objects.

`else` runs only after normal fallthrough from the protected body; its errors are handled outside that try statement. `finally` runs on normal fallthrough, error propagation, `return`, `break`, and `continue`. A new transfer from finally overrides the pending transfer and releases its owned return value or exception. Loop transfers inside finally use the block's lexical loop. Nested handlers restore the previously active exception.

Handlers cannot assume a fallible assignment completed. Definite assignment starts from the protected body's incoming state, excluding names that a nested handler could delete. Optional facts for potentially reassigned names are discarded. Finally is checked against a similarly conservative entry covering all exit reasons; initialize values before try when finally needs them. Integer interval facts in handlers and finally start unknown, and facts after try are conservative. These restrictions can reject safe programs requiring more precise exceptional-flow analysis.

`str(exception)` and `print(exception)` expose the message, including Python's special quoting for a one-string `KeyError`. Missing dictionary keys report the actual integer key or quoted string key. KeyError quoting follows Ruff's repr rules and pinned ICU Unicode data; newly assigned characters can print literally where a CPython build with older Unicode data emits an escape (for example U+1FAEA versus CPython 3.14). Other runtime-generated diagnostic messages are not a CPython text-compatibility promise; category matching and explicitly constructed messages are stable within this quoting rule. `assert` is always enabled independently of optimization, evaluates its optional string message only when false, and raises an empty-message `AssertionError` when no message is provided.

Standard output uses bounded buffering for pipes and files, and flushes each printed line on a terminal. Pending output is also flushed before `input()` waits, before a reported runtime error exits, and before the generated process entry returns normally. If a write or flush returns an error, Imre reports `OSError`; fatal-error flushing is best effort and preserves the original exception category. Generated code explicitly flushes at process exit because its LLVM entry point does not run Rust's standard `main` lifecycle.

Before creating file writers or initializing standard output, the runtime ignores `SIGPIPE` once so disconnected pipes produce catchable I/O errors. The sealed runtime has no user signal handlers or signal API.

## Link options

`imre build` and `imre run` remove unreachable code and data at every optimization level.
Imre passes `-dead_strip` to the macOS linker and `--gc-sections` to the Linux linker.
Use `--no-gc-sections` to keep unused code. On Linux, Imre also enables full RELRO so the
loader can make the global offset table read-only after resolving dynamic symbols at startup.
Select `-C relro-level=partial` for lazy binding or `-C relro-level=off` to disable RELRO.

Pass `-C link-arg=ARG` once for each extra argument to Clang, in command-line order.
Use Clang's `-Wl,` syntax for linker options. For example, this Linux build writes a link map
and prints the Clang invocation to standard error.

```sh
imre build totals.py -C link-arg=-Wl,-Map=totals.map --verbose-link
```

Set `IMREFLAGS` to whitespace-separated build options. For arguments containing spaces,
set `IMRE_ENCODED_FLAGS` to UTF-8 arguments separated by ASCII unit separators (`0x1f`).
When set, the encoded variable replaces `IMREFLAGS`, even when empty. Imre reads these
options before the command line. The last value wins for individual settings, and
repeated linker arguments accumulate in order. For example, this command enables section
GC for one build.

```sh
IMREFLAGS='--no-gc-sections' imre build totals.py --gc-sections
```

These variables apply to `build` and `run`. Use `--` on the `run` command line to pass
arguments to the compiled program.

## Incremental builds

`imre build` and `imre run` share a per-user cache with the bundled native
Linux toolchain and an ordinary runtime archive. Source builds on x86-64
Debian and Ubuntu can also use the cache with distribution Clang 18 and GNU
BFD as the system `ld`. Both commands
reuse the last executable when its sources, import lookups, build options, compiler,
runtime, and native link inputs are still current. Program arguments and the output
path can change between invocations.

The Linux releases use the private Clang and LLD 18 in `imre-toolchain/bin`.
Imre passes the bundled startup objects and libraries to Clang when linking for
the host. Executables use the host's glibc. On x86-64 Debian and Ubuntu, a source
build checks for `/usr/bin/ld.lld-18` and otherwise uses the system GNU linker.
`--verbose-link` shows the selected driver arguments. Changing linkers relinks
with the existing module objects.

LLVM's dependency file cannot represent every POSIX filename. When LLD reads an
object path containing a literal backslash or a custom linker script outside the
system library directories, Imre keeps the module objects and repeats the link
on the next invocation.

After a source edit, Imre checks and lowers the entire import graph. It recompiles
the modules whose emitted LLVM changed and relinks the program. Changing a public
signature or a shared layout can also recompile a caller. Changing only section GC
or RELRO relinks with the existing objects. `--verbose-link` performs the link
even when all the module objects are cached.

Build status goes to stderr. Use `-v` or `--verbose` to see each compiled and
reused module, or `-q` or `--quiet` to suppress progress. Set `NO_COLOR` for plain
terminal output. `--whole-program` optimizes all modules together and uses a
separate executable cache. `--no-cache` bypasses cache reads and writes while
preserving the selected code generation mode. These options also work in
`IMREFLAGS` and `IMRE_ENCODED_FLAGS`.

Custom Clang drivers and raw `-C link-arg` arguments, explicit targets, emitted
LLVM, sanitizers, ThinLTO, and unsupported toolchains use the whole-program
backend without caching. By default, cache data lives under
`$XDG_CACHE_HOME/imre/v1` or `~/.cache/imre/v1`. An absolute `IMRE_CACHE_DIR`
overrides the parent directory. Older objects stay available for reverts until
that cache directory is cleared.

## Build and validate

Install the Rust toolchain specified in `rust-toolchain.toml`, Clang 18, and CPython 3.12 or later. Build the compiler and runtime together:

```sh
cargo build --workspace --release --locked
target/release/imre check tests/programs/ownership.py
target/release/imre build tests/programs/ownership.py -o /tmp/ownership --emit-llvm /tmp/ownership.ll
/tmp/ownership
python3 tests/differential.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/imports.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/classes.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/dataclass_records.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/containers.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/comprehensions.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/unpacking.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/fstrings.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/calls.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/inference.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/iterables.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/luhn.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/expressions.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/exceptions.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/files.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/processes.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/paths.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/toml_records.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
python3 tests/io.py --compiler target/release/imre --runtime target/release/libimre_runtime.a
```

Build the compiler and runtime archive from the same revision. Imre finds the archive beside the compiler executable, or accepts an explicit path with `--runtime`. `--clang` selects the Clang driver and `--opt-level` selects 0 through 3 (default 3). Every input module, executable, runtime, explicit ThinLTO linker, and saved LLVM output must have a distinct file identity; symlinks and hard links cannot bypass the input overwrite check.

`--lto-linker PATH` enables optional cross-language ThinLTO and requires explicit `--runtime` and `--target TRIPLE`. Build that runtime with `RUSTFLAGS=-Clinker-plugin-lto` in a separate target directory, and select an LLVM linker that can read both the Rust and Clang bitcode. On Linux, use Clang 18 with the pinned Rust toolchain's bundled LLD. At optimization level 3, Imre raises LLVM's import and inlining budgets to 1,000 so runtime accessors can inline across the language boundary; this trades additional code size and link work for fewer runtime calls. Other optimization levels retain LLVM's normal budgets. `--target` also works without ThinLTO; it does not expand the supported runtime platforms.

On Apple Silicon, build the bitcode runtime with `--target aarch64-apple-darwin` so Cargo builds host tools normally. Pass the resulting `target/lto/aarch64-apple-darwin/release/libimre_runtime.a` to `--runtime`. Use the Mach-O linker `ld64.lld` from an LLVM release that reads both the Rust bitcode and your macOS SDK.

The following example builds the dependency planner on Linux.

```sh
RUSTFLAGS=-Clinker-plugin-lto cargo build -p imre-runtime --release --locked --target-dir target/lto
IMRE_TARGET="$(rustc -vV | sed -n 's/^host: //p')"
IMRE_LTO_LINKER="$(rustc --print sysroot)/lib/rustlib/$IMRE_TARGET/bin/gcc-ld/ld.lld"
target/release/imre build examples/dependency_planner/__main__.py \
  --module-root examples --clang clang-18 --target "$IMRE_TARGET" \
  --runtime target/lto/release/libimre_runtime.a --lto-linker "$IMRE_LTO_LINKER" \
  -o /tmp/dependency-planner-lto
```

All native test harnesses accept the same `--lto-linker` and `--target` options.

`imre build` and `imre run` use the system allocator by default. On Linux,
`--allocator jemalloc` selects the statically linked jemalloc heap for runtime
objects. `--allocator system` selects the platform heap explicitly. macOS and
`--sanitize` use the system heap. The compiler looks beside its executable
for `libimre_runtime.a` (system) or `libimre_runtime_jemalloc.a` (jemalloc).
An explicit `--runtime` selects the allocator through the archive itself.
For a Linux ThinLTO build with jemalloc, use `-p imre-runtime-jemalloc` in
the Cargo command above and pass `--runtime target/lto/release/libimre_runtime_jemalloc.a`.
Pass a system-allocator archive when supplying a custom runtime to `--sanitize`.

`--sanitize` marks generated functions for AddressSanitizer and links its runtime. On macOS, use `--clang` to select a Clang with a matching sanitizer runtime when you also instrument the Rust archive. To instrument Rust runtime code, build a separate sanitizer archive. The test suites accept `--sanitize --opt-levels 1` to exercise this configuration, including leak checks on normal exits.

The differential suite compares full stdout with CPython at `-O0` and `-O3`, covering 260 integer pairs, aliases, ownership across calls and replacement, evaluation order, short-circuiting, loops, and runtime errors. Separate rejection cases cover type violations, ignored errors, unsupported dynamic operations, and recursive calls. These are correctness checks, not performance measurements.

The imports suite compares multi-file programs with CPython at `-O0` and `-O3`, including package entry points, aliases, re-exports, relative imports, late function-global lookup, and mutation across module boundaries. It also checks invalid imports, unused invalid code, source diagnostics, and protection of imported files from output writes.

The classes suite checks nominal types, imported class aliases, field replacement, temporary receivers, returns, method argument evaluation, and normal-exit ownership at `-O0` and `-O3`. Rejection cases cover incomplete construction, escaping `self`, invalid writes, dynamic class features, recursive calls, and cyclic field types.

The dataclass records suite compares generated construction, keyword-only and fresh defaults, equality/repr, frozen writes, properties, class/static methods, `Self`, callbacks, imported decorator aliases, and typed JSON schemas. It checks argument/getter exceptions and receiver ownership. Rejections protect supported decorator options, exact types, and frozen/property mutation. The shared-NaN equality oracle requires CPython 3.13 or later; older hosts skip only that fixture because dataclass equality changed. The suite runs under ordinary optimization, ThinLTO, and ASan/LSan.

The containers suite covers nested lists, tuple positions and unpacking, list iteration and `enumerate`, optional ownership and local flow narrowing, contextual literals, shallow repetition, string concatenation, and whitespace splitting. Iteration cases exercise alias mutation, source and target rebinding, temporary roots, control flow, and arbitrary-precision counters. It rejects invalid alias writes, unsafe optional accesses, and class cycles hidden inside containers.

The comprehensions suite checks isolated and nested scopes, contextual result types, ordered filters and dictionary key/value evaluation, optional narrowing, alias-visible mutation, arbitrary-precision ranges, and cleanup after partial construction. It compares complete filesystem effects and rejects unsupported targets, escaped names, dynamic operations, and recursion hidden inside helpers.

The f-string suite compares literal decoding, raw and adjacent parts, scalar conversions, nested fields, optional narrowing, and comprehension captures. Fixed-point and percentage cases cover rounding ties, signed zero, nonfinite values, large integers, and the precision limit. Side-effecting fields, conversion overflow, later failures, mutable owners, and file writes check evaluation order and cleanup. Rejection cases cover unsupported formatting/conversions, debug fields, types, surrogate escapes, and builtin rebinding.

The truthiness suite compares concrete scalar/container truth, optional narrowing, operand identity, short-circuit writes and errors, and temporary ownership at `-O0` and `-O3`. It also checks exact result-type restrictions and unsafe optional/assignment facts, and runs with ThinLTO and AddressSanitizer in CI.

The unpacking suite compares chained assignment, nested/starred sequence patterns, field and subscript targets, constructor initialization, eager display expansion, alias mutation, partial failure, and resource cleanup. It also checks exact layouts, contextual empty values, optional facts, and comprehension target scope; the same cases run with ThinLTO and ASan/LSan.

The inference and iterable suites cover propagation through imports and methods, scalar defaults, missing or conflicting evidence, nullable returns, mutation during iteration, source evaluation order, conversion failures, and cleanup. The Luhn suite verifies byte-for-byte upstream sources, executes the original tests and doctests through native adapters, and compares all four string APIs against CPython over decimal digits, invalid inputs, UTF-8 errors, and repeated recovery. All three suites run under ordinary optimization, ThinLTO, and ASan/LSan.

The iterator-pipeline suite compares lazy generator evaluation, live enclosing bindings, private targets, assignment expressions, strict zip errors and probe order, reverse traversal, short-circuit truth tests, and ownership during mutation or exceptions. It compares full filesystem effects and checks that delayed writes cannot create unsafe definite-assignment or optional facts. The same cases run under ordinary optimization, ThinLTO, and ASan/LSan.

The calls suite compares positional/keyword binding, keyword-only parameters, scalar defaults, imported aliases, contextual container types, and optional values. Reordered arguments, temporary receivers, aliased owners, and later exceptions check evaluation order and cleanup. Rejection cases protect exact types, parameter kinds, required/duplicate names, literal defaults, and the positional builtin surface.

The callable suite compares function references, returned and optional callables, bound receiver ownership, noncapturing lambdas, mixed scalar/reference call signatures, and lazy map/filter composition with CPython. It exercises callback and argument failures, partial results, buffered file cleanup, source mutation, and short-circuiting. Rejections protect exact signatures, noncapturing scope, full positional arity, acyclic storage, and recursion restrictions. It shares the assignment suite's full stdout/filesystem oracle and runs under ordinary optimization, ThinLTO, and ASan/LSan.

The typed JSON suite compares native decoding with the checked CPython compatibility module. It covers exact scalar and nullable layouts, nested records/lists, required and defaulted fields, independent empty defaults, arbitrary integers, float rounding and overflow, Unicode, duplicate policy, depth limits, imported schema aliases, and recovery after partially allocated failures. Rejections protect structural constructors and the supported acyclic schema types.

The expressions suite compares lazy conditional arms, contextual result types, optional narrowing, Unicode string indexing and iteration, and list/string slices. It checks 1,701 slices apiece for strings and integer lists, including omitted and arbitrary-precision bounds. Aliased shallow copies, operand mutation, later failures, and recovery check ownership and evaluation order. Rejection cases protect exact types, fixed tuple bounds, and read-only slicing.

The floats suite compares arithmetic and mixed-number matrices, shortest formatting, strict decimal conversion, exact comparisons, and large rational rounding against CPython. Boundary cases include signed zeros, NaNs/infinities, the subnormal and overflow thresholds, and integers too large to convert individually. Concrete classes, tuples, lists, dictionaries, optional values, defaults, and exception cleanup exercise native storage; rejection cases forbid implicit annotation-driven conversions.

The exceptions suite compares nested propagation, handler ordering, builtin hierarchy, stored exception values, dynamically scoped re-raising, lazy assertion messages, and all finally exit reasons. Repeated failures during argument evaluation and partial container/class construction exercise ownership cleanup. Rejection cases protect definite assignment, optional facts, and the sealed exception contract.

The I/O suite checks complete output, large and Unicode writes, normal and fatal exit flushing, LF and CRLF pipe input, terminal line visibility, and output failures. Interactive checks use protocol handshakes and readiness deadlines rather than performance expectations.

The process suite compares complete captures and filesystem effects against CPython, including large dual-pipe output, inherited stdin, descriptor and signal handling, replacement environments, working directories, source-order argument mutation, status errors, and result ownership. Repeated failures run under a reduced descriptor limit; an inherited descriptor above that limit must still close in the child. Invalid Unix environment bytes exercise the explicit Unicode boundary. Rejections protect the exact captured-text mode and sealed result contract. Ordinary optimization, ThinLTO, and ASan/LSan use the same cases.

The paths suite compares lexical normalization, imports, value storage, filesystem predicates and errors, UTF-8 reads/writes, glob patterns, symlinks, pipeline evaluation order, snapshot timing, and complete filesystem effects. Descriptor limits and sanitizer runs cover early exits and repeated recovery. Run it with a CPython 3.14 or newer oracle (`--python python3.14`); earlier versions differ in suffix handling, directory acquisition timing, and failed filesystem predicates.

The workspace suite compares path mutations and scoped temporary directories against CPython 3.14+, including exact file bytes, permissions, hard-link relationships, and symlink targets. It exercises argument order, default-directory selection, aliases, mixed managers, all control transfers, permission repair, opaque Unix filenames, and cleanup failures that replace pending returns or exceptions. Rejections protect the typed arguments and direct-with-only constructor. Every case runs under a reduced descriptor limit and accepts the same optimization, ThinLTO, and sanitizer flags as the other filesystem suites:

```sh
python3 tests/workspaces.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --python python3.14 --clang clang-18
```

The files suite compares complete filesystem effects, strict decoding, newline handling, write counts, alias-visible closure, all `with` exit reasons, failed cleanup, argument aliases, and descriptor recovery under a reduced open-file limit. Rejection cases protect the sealed import, file, context-manager, and argument surfaces. The existing suites also run generated-code and runtime AddressSanitizer checks at `-O1` in CI.

The bytes suite compares full byte-range repr/indexing, strict UTF-8 decoding errors, constructors, exact typed storage and keys, 896 empty/nonempty slices, iterator ownership, and failures during partial construction at `-O0` and `-O3`. Binary fixtures compare complete file contents, append/exclusive modes, imported handle aliases, closed/wrong-direction errors, and every context-manager exit path. It accepts the same target, ThinLTO, and sanitizer options as the other native suites:

```sh
python3 tests/bytes.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang clang-18
```


### Closed entry profiles

`--reachable-only` checks and compiles definitions used by the entrypoint. It can resolve selected standard-library declarations, such as `configparser.ConfigParser`, when only an unused function needs them. Using one in executable code still requires a native implementation. Unreferenced first-party module values built solely from builtin literals may omit their storage if their keys are statically hashable and their annotations contain only static types. Other first-party initializers and modules with observable import effects remain subject to normal checks; unknown exports are rejected. `--entry-profile FILE` also selects particular branch bodies in named functions. The JSON file maps each qualified function name to the condition and value it requires.

```json
{"tools.report.run": {"mode": "Mode.markdown", "args.debug": "False"}}
```

A profile can select a named arm of a simple `match`, or exclude the true arm of a named Boolean condition. The executable evaluates the original test. Entering an excluded arm raises a `ValueError` with the required value. A missing function or unmatched selector is a build error.

### Context variables

`contextvars.ContextVar[T]` stores bindings by variable identity. The native
scheduler copies the current context when scheduling a task and restores it at
every resume; directly awaiting a cold coroutine shares the caller's context.
`ContextVar.get`, `set`, `reset`, token context managers, `copy_context`,
and `Context.run` support nested and exceptional execution.

Contexts support shallow copies, lookup, membership and size. Callback arguments
in `Context.run` currently need an exact positional signature. Thread startup,
explicit task `context=`, context iteration and mapping equality remain outside
the subset. See the [contextvars scope and CPython evidence](../benchmarks/cpython_stdlib/full/contextvars/README.md)
for value layouts and test roles.

### Native asyncio subprocesses

Typed coroutines support `await`, `asyncio.TaskGroup`, `asyncio.gather` and `asyncio.Semaphore` for subprocess-backed work. A coroutine starts when it is awaited or scheduled. A callback with an `Awaitable[T]` return type must be an `async def` function. A synchronous function that creates a coroutine can be called directly. A task retains its result across awaits. Exceptions returned by `gather(..., return_exceptions=True)` retain their positions among the results. Standard output and standard error are drained concurrently. Ruff’s live ecosystem checker also exercises loop signal handlers, `functools.cache`, suspended generator context managers, and `TemporaryDirectory` cleanup on exception paths.

## Native unittest cases and results

`from unittest import TestCase, TestResult, SkipTest` loads a small library
compiled into the program. A fixed-layout subclass can use `fail`, `skipTest`,
`countTestCases` and the default setup/teardown hooks. The boolean truth
assertions and integer equality and ordering assertions accept an optional
string message and honor the instance's `longMessage` setting. A result can
count started cases, record skips and unexpected successes, stop a run, and
receive run and test boundary notifications.

`with case.assertRaises(Expected, msg=...) as caught:` catches builtin and
declared exception categories using Imre's native exception hierarchy. It
accepts nested tuples and exposes the original exception as `caught.exception`.
A stored context can be entered later. Normal exit, return, break, and continue
raise the missing-exception assertion; unrelated exceptions propagate.
The callable form and runtime-selected exception classes need further support.

`unittest.main()` discovers the compiled entry module's statically bound
`TestCase` subclasses, including imports, aliases and inherited tests. It uses
the default lexicographic order and makes a separate instance for each method.
Its native runner calls `setUp`, the selected method and `tearDown`; a setup
failure prevents the latter two. Skipped tests, assertion failures and other
errors are reported separately. The default, `-v`, `-q` and `-f` modes return
normal unittest exit codes, including 5 for an empty suite. Only zero-argument
direct calls and the inherited initializer are supported. Custom loaders,
module and class hooks, decorated tests, and named command-line selections
require further support. Caught failures report their messages without CPython
traceback frames.

The [shared suite runner](../benchmarks/cpython_stdlib/SUITE_RUNNER.md) shows
which parts of a complete pinned test run natively. The full CPython unittest
API, dynamic method discovery in a native process, traceback reports, and
arbitrary assertion operand types are not supported by this implementation.

### Native errno registry

`import errno` exposes the target platform's integer constants and one mutable
`dict[int, str]` at `errno.errorcode`. Forward aliases stay distinct names;
the reverse dictionary uses the last registered name for each value.
Module aliases share the same registry and dictionary. `vars(errno)` and
`errno.__dict__` provide insertion-ordered key iteration, membership, and lookup.
The module supports dynamic `hasattr` and `getattr`, including scalar defaults.
Changing the module namespace and calling bound module descriptors are outside
this subset. The reverse dictionary supports ordinary insertion, replacement,
and deletion.

Native `TestCase.assertHasAttr` currently accepts a native module and a string;
`assertIn` accepts a scalar or native module member and a `dict[int, str]`.
Both use the normal assertion failure and optional test message. The errno
suite runs every original method; see the [reference and native evidence](../benchmarks/cpython_stdlib/full/errno/README.md).
