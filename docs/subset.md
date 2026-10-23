# Executable prototype contract

This is a deliberately restricted Python dialect for closed, statically typed source graphs, including selected unchanged pure-Python libraries. Relar targets 64-bit Linux with Clang 18 and Apple Silicon macOS with Apple Clang. Compilation uses Rust, Ruff, and ty; the executable links only the independent Rust runtime and system libraries. It does not import CPython or interpret unsupported operations.

## Accepted source

| Area | Supported behavior |
| --- | --- |
| Modules | Closed UTF-8 source graph; imports precede initialization; entry definitions precede executable statements; imported initializers run once in dependency/source order; optional docstrings |
| Imports | Absolute, `from`, and package-relative imports; aliases and package re-exports; fixed module, definition, and exported value bindings; shared mutable values; no import cycles |
| Functions | Exact signatures declared or inferred from concrete calls, scalar defaults, and checked bodies; direct positional and keyword calls; positional-only and keyword-only parameters; exact callable values, bound methods, and noncapturing lambdas; no recursive call cycles |
| Types | `int`, `float`, `bool`, `str`, `bytes`, `None`, sealed `pathlib.Path` and `subprocess.CompletedProcess[str]`, `typing.TextIO` and `typing.BinaryIO`, the sealed builtin `Exception` family, fixed nominal classes, string-valued `enum.StrEnum` and `typing.Literal`, concrete `list[T]`, `dict[K, V]`, and `set[T]`, fixed tuples such as `tuple[str, int]`, exact `typing.Callable[[T1, T2], R]` signatures, `str \| bytes`, and `T \| None`; locals have one inferred or declared storage type |
| Classes | Fixed fields, explicit or dataclass-generated initialization, typed instance/class/static methods, read-only properties, method-local `typing.Self`, alias-visible mutation, and frozen dataclasses |
| Statements | Single-target and chained assignment; nested and starred unpacking of fixed tuples, lists, strings, and bytes, annotated assignment, numeric augmented assignment, local string/bytes `+=` and bytes `*=`, dictionary entry deletion, `if`/`elif`/`else`, structural `match`/`case`, `while`, `for` over ranges, strings, bytes, concrete containers, or supported iterator pipelines, loop `else`, `break`, `continue`, `return`, `pass`, `assert condition[, string_message]`, `raise`, `try`/`except`/`else`/`finally`, and builtin file/workspace `with` |
| Conditions | Truth tests for every supported concrete type; operand-returning `and`/`or`, boolean `not`, and short-circuit evaluation; conditional expressions `yes if condition else no`; assignment expressions `(name := value)`; local optional narrowing |
| Integers | Arbitrary precision; `+`, `-`, `*`, `//`, `%`, `&`, `\|`, `^`, `<<`, `>>`, and `divmod`; `/` returns a correctly rounded float; unary `+`/`-`/`~` and `abs`; all six value comparisons, including chains |
| Floats | Binary64 literals and exact storage; `+`, `-`, `*`, `/`, `//`, `%`; unary signs and `abs`; all six comparisons; mixed integer arithmetic and exact mixed comparisons; explicit numeric/string conversions |
| Lists | One exact element type, including nested containers and classes; literals and starred displays, `list()` and `list(iterable)`, stable `sorted` with scalar or fixed-tuple keys, shallow concatenation, repetition, and slicing, checked negative indexing, indexed assignment, iteration, `append(value)`, `extend(source)`, `pop()`, `len(...)`; alias-visible mutation |
| Dictionaries | Exact `int`, `str`, string enum, `bytes`, `Path`, or immutable fixed-tuple keys and one concrete value type; literals and `**` displays, contextual `dict()`, lookup/write/delete, numeric augmented writes, `in`/`not in`, `get`, `len`, and insertion-ordered key/items iteration |
| Sets | Exact `int`, `str`, string enum, `bytes`, `Path`, or immutable fixed-tuple elements; literals and `*` displays, contextual `set()`, `add`, `discard`, `in`/`not in`, `len`, and iteration with unspecified order |
| Comprehensions | Eager list, dictionary, and set comprehensions; immediately consumed generator expressions; nested `for` clauses and truth-tested filters; isolated target bindings |
| Tuples | Fixed length and a type per position; literals, bounded literal indexing, nested/starred unpacking into names, fields, and subscripts, and `len(...)`; immutable slots; value equality for immutable field shapes and lexicographic ordering for ordered scalar shapes |
| Strings | Immutable UTF-8 text, literals, scalar, record, and list f-strings and `!r`, named `str.format` fields, signed decimal integer and bounded fixed/percent formats, `textwrap.dedent`, concatenation, equality/inequality, Unicode scalar length, indexing, slicing and iteration, bounded `split`/`rsplit`, `splitlines()`, stripping, prefix/suffix checks, `find`, `partition`, `isspace`, `join(iterable)`, `replace(old, new)`, input and output |
| Regular expressions | Fixed text patterns in `re.compile` and `re.search`, named and numbered groups, character offsets, and `re.VERBOSE` |
| Bytes | Immutable bytes; literals, construction from an integer count or `list[int]`, concatenation/repetition, all value comparisons, indexing/slicing/iteration, `len`, Python byte repr, strict UTF-8 encode/decode |
| Paths | Native POSIX `pathlib.Path`; lexical joining and components, equality, text I/O, filesystem predicates, directory creation/removal, unlink/replace, immediately consumed directory iteration and single-component globbing |
| Workspaces | Direct `with tempfile.TemporaryDirectory(...)` scopes with UTF-8 names, typed location/name options, and deterministic recursive cleanup |
| Files and arguments | Explicit UTF-8 text and binary file modes, buffered line iteration, binary `read(n)`, read-only `gzip.open`, shared cursors, `with` cleanup, and one shared mutable `sys.argv` list |
| Processes | Synchronous `subprocess.run` with string arguments, captured UTF-8 text, working directories, replacement environments, checked status, and `os.environ.copy()` snapshots |
| Builtins | `print(value)` for one scalar, path, exception, or record; `repr(value)` for supported data values; `input()` without a prompt; `bool()` or `bool(value)`; `int(int_or_float_or_bool_or_string)`; `float()` or `float(int_or_float_or_bool_or_string_or_bytes)`; `str()` or `str(int_or_float_or_bool_or_string_or_bytes_or_None_or_path_or_exception_or_record)`; `abs(int_or_float_or_bool)`; `len(string_or_bytes_or_container)`; `list([iterable])`; `sorted(iterable, key=callback_or_None, reverse=bool)`; integer `sum(iterable[, start])`; `min(integer_generator)` and `max(integer_generator)`; `any(iterable)` and `all(iterable)`; immediate `map(callback, iterable)`, `filter(predicate_or_None, iterable)`, `zip(iterable, ..., strict=condition)`, `reversed(sequence)`, and `enumerate(iterable[, start])`; contextual empty `dict()` and `set()`; `range` with one to three integer arguments at an iteration boundary |

Booleans have a separate exact type: use `int(flag)` to store one in an integer list. A fresh local initialized with `rows = []` can take its element type from the first direct `rows.append(value)` statement later in the suite, including inside a loop or conditional. This inference supports scalar `int`, `bool`, `float`, `str`, and `bytes` operands. An empty list otherwise defaults to `list[int]` unless a typed storage, call, or return boundary supplies its element type. No annotation permits `Any`, an unrestricted union, an untyped object, or a foreign container to enter native storage. ty checks ordinary Python typing first; Relar additionally enforces exact types, definite assignment, fixed bindings, and every container write. Suppression comments cannot waive Relar's contract.

Indentation must use spaces; tabs and form feeds remain valid inside string data. Physical CRLF and CR newlines are normalized to LF before parsing, including inside string literals, as in Python. An optional encoding declaration must spell `utf-8`, `utf_8`, or `utf8` (case insensitive). Other declared encodings and literal NUL bytes in source are rejected; escaped NUL characters in strings are supported.

The compiler bounds syntax complexity before parsing each module. Deeply nested blocks, delimiters, or long chains of operators can exceed the limit. Splitting a long expression into intermediate assignments reduces its depth.

Global rebinding, capturing closures, arbitrary decorators, reflection, dynamic attributes, generator functions, escaping iterator values, async, slice assignment/deletion, tuple display expansion, call argument expansion, variadic parameters, function defaults other than scalar literals, mutable-list augmented assignment, and arbitrary object identity comparisons are currently rejected. Unsupported operations are rejected even in dead branches and unused functions. A recursive call graph is rejected, including indirect recursion through callable values, methods, constructors, or comprehension bodies; a future recursion feature needs an explicit stack policy.

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

`sorted(source, key=callback, reverse=condition)` returns a fresh list. Both keywords are optional. `key=None` compares the elements themselves, and `reverse` defaults to false and requires an exact `bool`. Keys may be exact integers, booleans, strings, string enums, bytes, or fixed tuples recursively containing those scalar types. Tuple fields whose exact type is `None` are also allowed; nullable fields, floats, mutable containers, and records are not. Every key has the same concrete shape, and tuples compare lexicographically. Input and keyword expressions evaluate in source order, then the source is fully materialized before any key callback runs. Each key is evaluated once in input order. Sorting is stable in both directions, so equal keys retain their input order even with `reverse=True`. A callback may mutate the original source or its elements and may raise. The copy already contains all input elements when the first callback runs.

`values.sort(key=callback, reverse=condition)` accepts the same keys and updates the list in place. It returns `None`. Aliases observe the new order. During key evaluation the list appears empty until a callback adds elements. If a key raises, the list regains its original membership and order and the exception propagates. If keys succeed after a callback appends to the list, the sorted original elements are restored and the sort raises `ValueError`.

`sum(source[, start])`, including `start=`, consumes exact integers and defaults to zero. Its source can be any supported concrete iterable whose yielded values are integers, including fixed tuples such as `divmod(...)`. An immediate generator may also yield exact booleans. For example, `sum(file.passes for file in files)` counts the true values. The generator acquires its outer iterator before `start` runs, then evaluates each projection once. The start and the result are exact, arbitrary-precision integers, including for an empty generator.

`map(str, source)` supports scalar values, paths, and records with a checked string conversion. Conversion happens as each item is pulled.

An immediate `map(int, source)` converts each item using the ordinary checked `int` conversion. Inputs may yield exact integers, floats, booleans, or strings. Conversion proceeds lazily as the enclosing consumer iterates; input expressions, iterator creation, the optional sum start, and conversion failures retain Python evaluation order. Mutable sources remain live and keep the existing dictionary/set mutation checks. User callbacks follow the exact callable contract below. A map cannot escape into storage, and user-defined iteration protocols remain unsupported.

`itertools.chain` accepts a fixed number of positional iterables with one exact
element type. A loop, comprehension, or immediate consumer can use the chain.
It evaluates source expressions left to right, then acquires each iterator on
reaching its source. A second `Path.glob` sees directory changes made while
consuming the first glob.

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

A generator expression evaluates its leftmost iterable and acquires that iterator immediately. Its remaining iterables, filters, and element expression run only as the consumer requests values. Enclosing local bindings remain live between requests; generator targets have private scope. Assignment expressions write to the enclosing scope, but a delayed write does not prove the target initialized after a possibly empty or short-circuited consumer. `yield`, `yield from`, async clauses, and user-defined iteration protocols remain unsupported.

`zip` evaluates its arguments in source order and pulls inputs from left to right for each fixed tuple. It stops at the shortest input by default. Its optional `strict` keyword is truth-tested at construction; a true value raises `ValueError` when exhaustion reveals unequal lengths. Later inputs are probed only when required to detect a mismatch. A consumer that stops early can therefore finish before a mismatch becomes observable. Zero inputs produce an empty iteration.

`reversed` accepts concrete lists, Unicode strings, bytes, ranges, and homogeneous fixed tuples. A reversed list captures its starting index at construction, then reads live elements at descending indexes. Appended tail elements are not visited; an index outside the current list bounds exhausts the iterator when the next item is requested. String traversal yields Unicode scalar strings, and reversed ranges preserve arbitrary-precision bounds. Dictionaries, sets, generator expressions, and other iterator pipelines cannot be passed to `reversed`.

`min(expr for item in source)` and `max(expr for item in source)` consume a single generator of exact integers. They evaluate each yielded value once and keep the first minimum or maximum. An empty generator raises `ValueError`.

`any` and `all` truth-test yielded values and return a boolean. `any` stops at the first true value and returns false for an empty input; `all` stops at the first false value and returns true for an empty input. Skipped elements, conversions, filters, and strict-zip probes do not execute. All source expressions still must type-check.

## Typed JSON boundaries

`json.loads(text)` accepts a Unicode string containing a JSON array. Iteration
preserves input order. A function can receive a decoded object as
`dict[str, Any]` and read nested literal string keys. Selecting into an
annotated `str`, `int`, or another object checks that exact type at the read,
including the nullable forms. Integers retain their full precision. A missing
key raises `KeyError`; a value of the wrong type raises `TypeError`. Duplicate
keys use the last value. Comparing a selected value with a string literal
supports filters such as `if row["severity"] == "major"`. The object view is
read-only. Dynamic keys, decoder options, and general `Any` annotations are
outside this boundary. Malformed JSON raises `json.JSONDecodeError` (a
`ValueError`), and nesting is limited to 128 containers. Strings use Unicode
scalar values, and numeric literals follow the standard JSON grammar.

`from relar.json import decode, encode` exposes explicit Relar APIs. `decode(text, Schema)` takes a string and a statically resolved record class, `list[T]`, or `dict[str, T]`, returning that exact type. Scalar and optional roots are not accepted by the decoder. Import aliases and first-party re-exports preserve the intrinsic identity. The native decoder constructs existing typed records, lists, and dictionaries directly, without exposing unknown values or `Any`.

```python
from dataclasses import dataclass, field
from relar.json import decode, encode

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

The checked CPython implementation lives in `stdlib/relar/json.py`. Add `stdlib` to Python's import path when running the same source, for example `PYTHONPATH=stdlib python3 program.py` from this repository. The native compiler recognizes the sealed import without executing that module. It rejects a first-party file that shadows the intrinsic namespace.

## Typed TOML boundary

`tomllib.load(stream).keys()` accepts a binary file at its current position. It
reads through EOF and validates the entire document under the TOML 1.0 grammar
used by CPython 3.14. The result can be annotated `collections.abc.Set[str]`.
It supports string membership, length, truth testing, and iteration in source
insertion order after the stream is closed. Invalid UTF-8 raises
`UnicodeDecodeError`. Syntax errors raise `tomllib.TOMLDecodeError`, a
`ValueError` subclass. The native reader bounds TOML nesting at 64 containers.

`from relar.toml import decode` provides `decode(text, Schema)` for complete TOML 1.1 documents. The schema is a statically resolved record class or `dict[str, T]`; TOML's document root is always a table. Import aliases, module imports, and first-party re-exports work as they do for typed JSON. The result uses ordinary concrete records and containers, with no exposed dynamic TOML value type.

```python
from dataclasses import dataclass
from relar.toml import decode

@dataclass(slots=True)
class Group:
    name: str
    href: str

groups = decode('[concepts]\nname = "Concepts"\nhref = "/concepts"',
                dict[str, Group])
print(groups["concepts"].name)
```

Nested schemas support exact `int`, `float`, `bool`, and `str` values, lists, string-keyed dictionaries, fixed records, and their supported optional forms. Records share typed JSON's structural-constructor checks, dataclass support, and native field layouts. Arrays of tables decode as lists of records or dictionaries. Dictionaries retain source insertion order. Bytes, tuples, sets, arbitrary unions, resource handles, and datetime annotations are unsupported.

TOML scalar categories remain distinct: booleans cannot satisfy integer fields, and integer tokens cannot satisfy float fields. Decimal, hexadecimal, octal, and binary integers retain arbitrary precision through Relar's canonical integer representation. Floating-point tokens produce binary64 values, including signed zero, `inf`, and `nan`; overflow follows CPython's conversion to infinity. CPython's maintained parser retains its configured decimal integer conversion limit (4300 digits by default); native decoding has no corresponding digit limit. Applications that need larger decimal inputs under CPython can configure its standard integer conversion guard before decoding.

A missing field requires a constructor default, even when its annotation is optional. Scalar defaults and fresh list defaults follow the typed JSON rules. An optional list with a list factory receives a fresh `[]` when absent; an optional field defaulting to `None` receives `None`. TOML has no null token. Present values must match the optional payload's exact type. Decoding applies the compiler-proved field writes without executing arbitrary constructor code.

The complete document is parsed before schema conversion. Unknown record fields are ignored after validation, and duplicate keys or tables are rejected even in ignored data. Valid date, local time, local datetime, and offset datetime values can appear in ignored fields; they cannot be exposed as strings, records, or dictionaries through a schema. Calendar validation follows CPython's representable range, excluding year zero and leap-second values. Invalid syntax, schema mismatches, missing required fields, and more than 64 nested table/array containers raise `ValueError`; the depth count includes the document table and ignored data. Error wording is not a parser-library compatibility promise.

The CPython module is `stdlib/relar/toml.py`. Add `stdlib` to the import path as for typed JSON. Python 3.15+ uses its TOML 1.1 `tomllib`; earlier versions require `tomli>=2.4`, for example `python -m pip install 'tomli>=2.4'`. Native compilation uses the pinned Rust TOML parser and links no Python interpreter or compatibility module. This boundary currently provides decoding only.

## Native text operations

`str.split(sep=None, maxsplit=-1)` and `str.rsplit(sep=None, maxsplit=-1)` accept both positional and named arguments. With no separator, they group Python whitespace and omit empty fields; an explicit string separator preserves empty fields and must be nonempty. The split limit follows Python's left or right scanning rules. `str.strip([chars])`, `str.lstrip([chars])`, and `str.rstrip([chars])` accept an optional positional string or `None`: omitted/`None` removes Python whitespace, while a string names a set of Unicode scalar values to remove.

`str.startswith(prefix)` and `str.endswith(suffix)` accept one positional string, without tuple alternatives or bounds. `str.find(sub[, start])` accepts a positional substring and an optional integer or `None` start, returning a Unicode scalar index or `-1`; an end bound and `rfind` are not yet supported. `str.partition(sep)` requires a nonempty positional string and returns a fixed `(before, separator, after)` tuple; a missing separator returns `(original, "", "")`. `str.isspace()` is false for an empty string and otherwise checks every character against Python's whitespace definition. These operations use native UTF-8 storage and do not import or call Python methods.

`urllib.parse.unquote(string)` is a sealed native import for one positional or named string argument. It decodes percent escapes with the default UTF-8 replacement behavior. Byte inputs, configurable `encoding`/`errors`, and other URL parsing APIs remain unsupported.

## Fixed regular expressions

`re.compile` accepts a literal string and an optional flag of `0` or `re.VERBOSE`.
`re.search(pattern, text)` accepts a literal or a compiled `re.Pattern[str]`.
A compiled pattern also has a `search(text)` method. The compiler checks each
literal, then the executable initializes each distinct translated pattern once.
A module-level declaration before executable statements can be shared by
functions and imported by other modules. Rust’s `regex` and `regex-automata`
crates implement native matching.

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
returns a string. Other groups return `str | None`, preserving the difference
between an empty capture and an optional group that did not participate.
Offsets count Unicode code points. An unmatched group's offsets are `-1`,
and an unknown group raises `IndexError`. The match retains its original text.

The supported dialect includes literals, dot, simple character classes and
ranges, alternation, named and numbered captures, noncapturing groups, and
greedy or lazy repetition. `\s` and `\S` use Python's Unicode whitespace.
Verbose patterns ignore ASCII layout and comments outside character classes.
Use `\A` or `^` for the start and `\Z` or `\z` for the strict end of the
string. The checker rejects dynamic patterns, bytes, additional flags,
backreferences, lookaround, `$`, word and digit shorthands, and repetitions
whose capture or empty-match behavior falls outside this dialect.

## Bounded unions and codec primitives

`str | bytes` and `typing.Union[str, bytes]` denote the same sealed two-member type; either can additionally include `None`. Function, return, local, field, tuple, and container storage enforce those exact members. Concrete strings and bytes promote into union storage without copying or allocating a wrapper. This does not allow arbitrary unions or heterogeneous untyped containers.

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

A loop can search a fixed tuple of up to eight differently typed fields. The tuple evaluates once before the first test. Relar checks the fresh loop variable at the corresponding field type each time.

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

Destination annotations can type empty source literals and promote copied values into optional storage. Existing source containers keep their own exact layout and aliases. Tuple result displays, argument expansion in calls, variadic parameters, and user-defined iteration protocols remain outside this milestone.

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

A direct call can pass an exact `int` to a `float` parameter when that parameter appears only in two-operand comparisons with the integer literal `0`. For example, a function that tests `delta == 0` and `delta > 0` accepts integer counts of any size. Relar evaluates the argument once and compares the integer exactly to obtain its sign. Float arguments retain their original binary64 values, including NaN and signed zero. The proof requires at least one comparison and excludes rebinding, captures, and uses of the value elsewhere. Other `float` parameters, calls through `Callable` values, and defaults require exact floats.

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

`relar check` and `relar build` resolve imports under one module root. By default this is the entry file's directory, or the parent of its outermost package when enclosing directories contain `__init__.py`. `--module-root DIRECTORY` selects the root explicitly. An entry inside a package follows `python -m package.program` semantics: enclosing package initializers are loaded and relative imports are valid. A package `__main__.py` can be the entry; `__init__.py` itself cannot.

For example, `import package.operations` binds `package`, `import package.operations as operations` binds the child module, and `from package.operations import solve as run` binds the same function under `run`. Module-qualified calls and imported calls resolve to their defining module's namespace. Imports precede executable initialization. Imported-module definitions may follow value assignments; calls made during initialization must only reach definitions and globals already initialized at that point, including through methods, constructors, and comprehension helpers. Fixed module, definition, and exported value bindings cannot be reassigned or shadowed by parameters or locals.

Imported modules, including package `__init__.py` files, run their typed top-level code once before their importer continues. Dependencies initialize in import order, and each initializer follows source order. Direct unconditional `NAME = expression` and `NAME: T = expression` assignments create typed exported value bindings. Aliases and re-exports share the same storage, so mutations to an imported dictionary remain visible in the defining module. Chained, unpacked, or conditional initializer locals are not exported. A single-name assignment in the entry module before a later function or class creates a fixed global. Its initializer runs once in source order. Subsequent executable statements use entry locals. A module can import postponed annotations and read its own module docstring through `__doc__`. Initialized global owners are released on normal and exceptional process exits.

An optional `if __name__ == "__main__":` guard without an `else` is supported. Every loaded module is type-checked and lowered for validation, including unused functions and inactive guards. The fixed `__name__` string is `"__main__"` in the entry and the qualified module name in imported code.

Imports are resolved statically from `.py` files and packages with `__init__.py`. Wildcards, dynamic or function-local imports, namespace packages, import cycles, dependencies outside the module root, and general standard-library imports or shadowing are rejected. The compiler recognizes sealed bindings for `itertools.groupby`, `enum.StrEnum`, `enum.auto`, `typing.Literal`, `typing.assert_never`, `typing.TextIO`, `typing.BinaryIO`, `typing.Union`, `typing.Callable`, `typing.Self`, `collections.abc.Callable`, `dataclasses.dataclass`, `dataclasses.field`, `pathlib.Path`, `tempfile.TemporaryDirectory`, `subprocess.run`, `subprocess.CompletedProcess`, `subprocess.SubprocessError`, `subprocess.CalledProcessError`, `subprocess.TimeoutExpired`, `os.environ` for `copy()`, `sys.argv`, `io.UnsupportedOperation`, `urllib.parse.unquote`, `relar.json.decode`/`encode`, and `relar.toml.decode`; these do not execute standard-library Python code. Their import aliases and first-party re-exports preserve identity. A package can re-export definitions from its children; reading an unfinished parent's definitions through a circular import is rejected. Symlinks cannot escape the module root or give one source file multiple module identities. The source-string library APIs accept sealed bindings but remain isolated from filesystem imports, which use the path-based APIs.

The I/O surface is intentionally small. `input()` reads a UTF-8 line and removes its trailing LF. A preceding CR remains part of the value, matching CPython's piped input on both hosts. `int(str)` accepts signed decimal digits, optional underscores between digits, and surrounding Python numeric whitespace. Decimal classification is pinned to Unicode 16, matching CPython 3.14; mixed decimal digit families are accepted, while superscripts and other numeric lookalikes are rejected. Other bases remain unsupported. Surrogate escapes in non-raw string literals are rejected: Ruff's analysis AST substitutes replacement characters for them, which is unsuitable for executable semantics. Strings support `+` and local `+=`; `text.split()` groups Python whitespace and omits empty fields, returning `list[str]`. `text.splitlines()` recognizes LF, CR, CRLF, vertical tab, form feed, U+001C through U+001E, U+0085, U+2028, and U+2029. It returns no elements for empty input and no extra element for a final line break; U+001F remains ordinary text. Additional string operations and their accepted signatures are listed under [native text operations](#native-text-operations). Retained line endings, general formatting protocols, and arbitrary codecs remain unsupported. CPython's configurable integer conversion digit limit is not reproduced.

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

Variadic parameters and `*`/`**` argument expansion remain unsupported. Builtins and methods accept specific keywords. `open` accepts `encoding="utf-8"`, `sum` accepts `start`, `zip` accepts `strict`, and both `sorted` and `list.sort` accept `key` and `reverse`.

## F-strings and repr

F-strings interpolate exact `int`, `float`, `bool`, `str`, `bytes`, `None`, sealed builtin exception values, fixed records, and lists of values with a supported representation. Ordinary, raw, triple-quoted, and adjacent literals are supported, including escaped braces, Unicode, and escaped NULs. Each field expression and its conversion complete before the next field evaluates. Nested f-strings may appear inside field expressions. Optional values require the same local narrowing as other scalar operations.

```python
def describe(name: str, quantity: int) -> str:
    return f"{name}: {quantity} units"
```

The default conversion and `!s` use scalar conversion or the fixed record’s `__str__` method. Records without that method use their generated representation. A list renders each element with its repr, including for `f"{rows[:5]}"` and `f"{rows!s}"`. `!r` and `repr(value)` additionally format supported containers, tuples, unions, and optional data, recursively composing generated dataclass repr. Strings use Python quoting and escaping. Resources, exceptions, and callables have no supported repr. The existing restriction against rebinding builtin names still applies. An absent or explicitly empty format specification, as in `f"{quantity:}"`, is supported.

Numeric fields additionally accept literal `f`, `%`, `.Nf`, and `.N%` specifications, with decimal precision `N` from 0 through 1000. Omitted precision is six. All four forms accept a leading `+`, `-`, or space sign option, as in `f"{change:+.2%}"`. Integer and boolean values convert to binary64 using the ordinary checked conversion, so an oversized integer raises `OverflowError` before any later field runs. Fixed-point formatting rounds ties to even and preserves signed zero. Percentage formatting first multiplies the binary64 value by 100 and appends `%`; infinities and NaNs use lowercase spellings. For example, `f"{ratio:.2%}"` and `f"{bytes_used / 1024:.2f}"` are supported. Numeric specifications cannot follow `!s` or `!r`.

Integer and boolean fields support decimal `d` and the `+`, `-`, and space sign options with or without `d`. For example, `f"{change:+}"` produces `+0`, `+3`, or `-3`. Decimal formatting preserves arbitrary integer precision and formats booleans as zero or one.

Dynamic specifications, width, alignment, other numeric presentation types, `!a`, and debug `=` fields are rejected, including `f"{quantity=!s}"`. Records support a statically resolved `__str__` method. Custom `__format__` and `__repr__` methods are unsupported.

Raw prefixes apply only to their own literal text. Expressions inside a raw f-string still follow their ordinary string rules, and adjacent parts keep their individual prefixes. Surrogate escapes in non-raw literal text or field expressions are rejected consistently with ordinary strings; tabs and form feeds inside literal text remain data.

A string template can substitute named `str` and `int` arguments, as in `url.format(filename=file, line=42)`. The template may be computed at runtime. Arguments evaluate once in call order, including unused arguments. Repeated fields reuse their value and `{{` and `}}` produce literal braces. An empty specification and `!s` are supported. A missing name raises `KeyError` and an unmatched brace raises `ValueError`. Positional call arguments, field traversal, and the rest of the format mini-language are outside this subset.

`textwrap.dedent(text)` removes the common prefix of spaces and tabs on nonblank lines. Each tab counts as a distinct character. Whitespace-only lines become empty as in Python 3.14. Interpolation happens before dedenting, so newlines in field values participate in the common-prefix calculation.

## Immutable bytes

`bytes` is an exact immutable type distinct from `str`. Ordinary, raw, escaped, and adjacent byte literals use Python's literal rules. Every byte from 0 through 255 is valid, including NUL and invalid UTF-8. Bytes can appear in classes, tuples, lists, dictionary keys/values, sets, and optionals. Literal bytes are also accepted as immutable parameter defaults.

`bytes()` returns empty bytes; `bytes(count)` creates that many zero bytes; `bytes(list_of_ints)` checks each integer is in 0 through 255; and `bytes(existing_bytes)` preserves its value. Counts must fit a native signed index before the nonnegative check: negative representable counts raise `ValueError`, unrepresentable counts raise `OverflowError`, and out-of-range list values raise `ValueError`. Booleans do not satisfy these exact integer inputs. Other iterable constructors and the string-plus-encoding constructor are outside this milestone.

Indexing uses exact integer indexes, including negative indexes, and returns an `int` in 0 through 255. Out-of-range indexes raise `IndexError`. Slicing follows the sequence rules above and returns bytes. Iteration, comprehensions, and loop-head `enumerate` yield integers in byte order while retaining the original immutable source independently of rebinding. Concatenation requires two byte values; repetition accepts bytes and an exact integer in either order. Repetition converts the count to a native signed index even for empty input and rejects an overflowing result. All six comparisons use unsigned-byte lexicographic order. `print`, `str`, and scalar f-strings use Python's `b'...'` representation, including quote selection and hexadecimal escaping.

`text.encode()` produces UTF-8 bytes; `data.decode()` performs strict UTF-8 decoding. They accept no arguments or keywords in this milestone. Decode errors raise `UnicodeDecodeError`, including Python's byte positions/ranges and invalid-start, invalid-continuation, or truncated-sequence message. Existing strings contain only Unicode scalar values, so encoding is always valid. Mutable `bytearray`, memory views, implicit text/byte conversions, additional codecs, error policies, and other byte methods are not supported.

## Native paths and directory traversal

`from pathlib import Path` exposes a sealed native POSIX path type on both supported hosts. `Path()` represents the current directory; fixed positional string or `Path` arguments join left to right. The `/` operator accepts a path and a string or path, including a string on the left. An absolute segment replaces the preceding path. Empty segments, repeated separators, and `.` components normalize lexically; `..` components and exactly two leading slashes remain intact. Construction and joining do not access the filesystem or resolve symlinks.

`name`, `stem`, `suffix`, and `parent` are read-only properties. Parent traversal stops at the anchor or current directory. Components follow [Python 3.14 semantics](https://docs.python.org/3.14/library/pathlib.html), including `Path("report.").suffix == "."`. On these POSIX hosts, `path.as_posix()` returns the same text as `str(path)`. Paths compare by normalized value with `==` and `!=`; `str`, `print`, and ordinary f-strings produce the path text, while `repr` and `!r` produce `PosixPath(...)`. Paths are always true, including `Path("")`. They can appear in exact fields, lists, dictionary keys and values, sets, tuples, callables, and nullable storage. Dictionary and set lookups hash the normalized path spelling without accessing the filesystem. Dataclass equality and repr traverse their native values. Path ordering, subclasses, and arbitrary `os.PathLike` implementations are outside this contract.

```python
from pathlib import Path

root = Path("reports")
for path in sorted(root.glob("*.json"), key=lambda path: path.name):
    print(path.stem)
    print(path.read_text())
```

`exists()`, `is_file()`, and `is_dir()` follow symlinks and return false for failed filesystem queries, matching Python 3.14, including invalid NUL-containing paths. `read_text()` reads and closes a file with strict UTF-8 decoding and universal-newline translation, regardless of the process locale. An explicit `encoding="utf-8"` has the same effect. `write_text(text, encoding="utf-8")` requires the literal encoding keyword and creates or truncates the file, writes UTF-8, closes, and returns the Unicode scalar count. Both methods use the same catchable exception categories as `open` for access and close failures. Argument evaluation completes before opening a file, so an argument failure cannot truncate the output.

`mkdir(mode=0o777, parents=False, exist_ok=False)` creates a directory. The mode is an exact integer within the signed C-int range; larger values raise `OverflowError`. Boolean options require exact booleans. The process umask applies, and missing parents use mode `0o777` independently of the leaf mode. `exist_ok=True` accepts an existing directory, including a symlink to one, without changing its permissions. `unlink(missing_ok=False)` removes a file or symlink; its flag suppresses only `FileNotFoundError`. `rmdir()` removes an empty directory. These methods return `None` and preserve their normal filesystem error categories.

`absolute()` prefixes a relative path with the working directory and preserves symlinks and `..` components. `is_absolute()` checks the lexical anchor without accessing the filesystem. `resolve(strict=False)` returns a new absolute path, resolves symbolic links, and removes `..` components in filesystem order. Its `strict` argument is an exact boolean and may be positional or named. With `strict=True`, missing components, non-directory traversal, permission failures, and symlink loops retain their operating-system exception categories. The default resolves as far as possible, appending unresolved components while still normalizing parents. Acyclic symlink chains are not limited by the kernel's per-system-call symlink count. Argument evaluation precedes filesystem access. Resolution requires UTF-8 paths and symlink targets; `absolute`, `relative_to`, user expansion, and general path protocols remain unsupported.

`replace(target)` accepts a string or native path, performs POSIX replacement, and returns the destination `Path`. Relative targets refer to the process working directory. The original path value remains unchanged. A string target reaches the filesystem before its returned path is normalized, so a trailing slash retains its operating-system meaning. Existing files or empty directories can be replaced, subject to normal type and filesystem restrictions; cross-filesystem replacement fails. The rename is atomic on one filesystem, but does not promise durable storage. All mutation arguments evaluate in source order before the operation; positional and keyword calls follow the corresponding Python signatures.

`iterdir()` and `glob(pattern)` are supported at immediate iteration boundaries, including loops, comprehensions, generator pipelines, `list`, `sorted`, `map`, `filter`, `enumerate`, `zip`, `any`, and `all`. They yield exact `Path` values and never include `.` or `..` directory entries. Iteration order is unspecified; sort by a scalar key when order matters. `glob` accepts one nonempty literal POSIX component containing ordinary names, `*`, `?`, or bracket character classes. Matching is case-sensitive and includes leading-dot names, directories, and broken symlinks. Separators, `**`, optional glob flags, recursive traversal, and escaping iterator values are rejected.

Directory acquisition follows Python 3.14: calling `iterdir()` immediately snapshots directory entries and closes the scan, while `glob()` defers filesystem access until its consumer requests a value. A wildcard glob snapshots and closes the directory scan before yielding its first result. Later body mutations do not extend that snapshot. `iterdir` propagates scan failures; `glob` suppresses operating-system scan failures, but other errors retain their normal category. Hidden traversal owners are reclaimed after exhaustion, early exit, and exceptions.

Native strings contain Unicode scalar values and paths use UTF-8. A directory entry with undecodable filename bytes raises `UnicodeDecodeError`; Unix surrogateescaped filenames are outside this domain. Lexical operations retain embedded NULs, while accesses that require a valid OS path raise `ValueError` unless the predicate or glob behavior above suppresses that particular failure. No operation silently substitutes replacement characters.

## Scoped temporary directories

[`tempfile.TemporaryDirectory`](https://docs.python.org/3.14/library/tempfile.html#tempfile.TemporaryDirectory) is supported directly in a `with` item. Its keyword-only options are `dir=None`, `prefix="tmp"`, and `suffix=""`. The directory accepts a string, native path, or nullable form; prefix and suffix require exact strings. The optional `as` target is one string name. Import aliases and first-party re-exports retain the sealed constructor identity.

```python
from pathlib import Path
from tempfile import TemporaryDirectory

output = Path("report.md")
with TemporaryDirectory(dir=output.parent, prefix=".relar-") as name:
    staged = Path(name) / "report.md"
    staged.write_text("Complete report\n", encoding="utf-8")
    staged.replace(output)
```

Creation chooses an unused name, requests mode `0o700` subject to the process umask, and returns an absolute pathname. Prefix and suffix retain ordinary POSIX joining behavior, including separators. An omitted directory uses the first usable candidate from `TMPDIR`, `TEMP`, `TMP`, the standard temporary locations, and the current directory; this choice is cached for the process as in CPython. Creation failures propagate before the body starts.

Cleanup removes the directory tree on fallthrough, return, loop transfers, and exceptions. A hidden owner retains the original pathname even if the visible target is rebound. Multiple file and workspace managers enter left to right and clean up in reverse order; failure to create or enter a later manager still cleans up earlier ones. A cleanup error replaces an earlier exception or pending return, while outer cleanup continues. Returned or stored strings remain ordinary values after the directory is removed.

Missing entries and an already-removed root are tolerated. Descendant symlinks are unlinked without following them; a root replaced by a symlink or regular file raises an error. Cleanup repairs owner permissions where CPython does and traverses raw Unix entry names without decoding them into language strings. This permits removal of an opaque filename created by an external process while preserving the Unicode boundary for public path operations. Moving the entire root elsewhere leaves that moved tree outside the original scope's cleanup path.

Temporary-directory objects cannot be stored, passed, returned, manually entered, or inspected. `delete`, `ignore_cleanup_errors`, positional constructor arguments, arbitrary context-manager protocols, and finalizer-driven cleanup are unsupported. Use the scope to own resources and ordinary `str`/`Path` values to name them.

## UTF-8 files and process arguments

`open(path, "r", encoding="utf-8")` and `open(path, "w", encoding="utf-8")` return a sealed `typing.TextIO` handle. The path is a string or native `Path` expression; mode and encoding must be the shown literals. Read mode opens an existing file; write mode creates or truncates a file. There is no locale-dependent default encoding. Handles can be passed, returned, stored in fields or containers, and aliased; every alias observes the same open/closed state.

`read()` consumes the remaining file and decodes strict UTF-8, translating CRLF and bare CR to LF. Further reads at EOF return the empty string. `write(text)` writes UTF-8 and returns the number of Unicode scalar characters, including embedded NULs. `flush()` makes buffered output visible to the operating system; it does not promise durable storage. `close()` flushes and closes, marks the shared handle closed even on a close failure, and succeeds on repeated calls. `.closed` is read-only. Entering, reading, writing, or flushing a closed handle raises `ValueError`. Reading a write-only handle or writing a read-only handle raises `io.UnsupportedOperation`, which matches both `OSError` and `ValueError`.

`open(path, "rb")`, `open(path, "wb")`, `open(path, "ab")`, and `open(path, "xb")` return a sealed `typing.BinaryIO` handle. Binary calls accept no encoding or other keyword arguments. Read mode opens an existing file, write mode creates/truncates, append mode creates if absent and appends, and exclusive-create mode raises `FileExistsError` if the path already exists. `read()` returns all remaining raw bytes without newline conversion; `write(bytes)` returns the number of bytes written. EOF returns `b""`. Binary handles support the same shared ownership, `flush`, `close`, `.closed`, and context cleanup as text handles. Their access mode survives closure: reading a write-only stream or writing a read-only stream raises `io.UnsupportedOperation` even after it closes; supported operations on a closed stream raise `ValueError`.

`with` accepts these builtin handles and the temporary-directory scopes above, with optional name-only `as` targets and multiple managers. It evaluates and enters them left to right and cleans them up in reverse order. Cleanup runs on fallthrough, errors, returns, and loop transfers. A hidden owner retains the entered file independently of any rebinding of its visible target. A later manager's failure closes earlier files and removes earlier workspaces. A close failure replaces an earlier exception or return, while outer cleanup still runs. The target remains bound after normal exit, usually to the now-closed file.

Both reader types support `readline()` and line iteration. Text reads incrementally decode strict UTF-8 and translate CR, LF, and CRLF to LF, even across buffer boundaries. Binary lines retain their bytes and end at LF. A line includes its terminator when one exists. An empty read signals EOF, so a blank line and the last unterminated line are both observable. Aliases, `for` loops, comprehensions, and explicit reads advance the same cursor. Breaking a loop leaves the handle available to its owner. Binary `read(n)` accepts an exact integer, reads at most that many bytes, and returns the remainder for `-1`. Zero requests no bytes. Smaller negative sizes raise `ValueError`, and sizes outside the native signed index range raise `OverflowError`.

`gzip.open(path, "rb")` and `gzip.open(path, "rt", encoding="utf-8")` accept a string or native path and expose the same buffered read operations. Concatenated gzip members form a single stream. Consuming a malformed header or trailer can raise `gzip.BadGzipFile` (an `OSError`), a truncated member can raise `EOFError`, and invalid compressed data can raise `zlib.error`.

Final reference release closes a file best-effort without callbacks or a new pending exception. Use explicit `close` or `with` to observe close errors. Text append modes, update modes, seeking, sized text reads, gzip writing, arbitrary codecs, newline options, generic context-manager protocols, and user finalizers are outside this contract.

Invalid UTF-8 raises `UnicodeDecodeError`, matching `UnicodeError` and `ValueError`. Embedded NULs in paths raise `ValueError`. Missing files, permission failures, directories opened as files, and non-directory path components use `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`; exclusive-create collisions use `FileExistsError`, also an `OSError`; other operating-system errors use `OSError`. Runtime-generated message text is not a CPython compatibility promise.

`sys.argv` is one lazily initialized mutable `list[str]`, shared across aliases and modules. Its first element identifies the invoked native executable, corresponding to the script identity under CPython. Element writes and list mutation are supported; rebinding the module attribute or imported name is not. Native arguments must be valid UTF-8. Invalid Unix argument bytes raise `UnicodeDecodeError` when the list is first accessed; CPython's surrogateescaped strings are outside this runtime's string domain. Programs that never read process arguments emit no argument initialization or shutdown calls.

## Command-line options

A function can declare an `argparse.ArgumentParser`, add literal long options in order, and return the result of `parse_args()`. Relar derives a read-only namespace with a concrete field for each option. Hyphens become underscores in field names. A choice of strings gives the field a string `Literal` type. A value option without a default or `required=True` produces an optional field.

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

The accepted declarations use one ASCII long option per `add_argument`, `str` or `Path` conversion, a matching literal default, `None`, `required`, `nargs="+"`, or `action="store_true"`. A list or tuple supplies constant string choices. Constructor options include a literal `prog`, a literal description or `__doc__`, and `RawDescriptionHelpFormatter`. In the default configuration the displayed program name is the entry script's basename. The parser reads the current `sys.argv[1:]` and handles repeated options, unique long-option abbreviations, and `--`. Help layout and coloring follow CPython 3.14. Help and usage errors follow the native exception cleanup path and raise catch-only `SystemExit` with status 0 or 2.

Keep the builder local, with its declarations immediately followed by an assignment or return of `parse_args()`. Positionals, user-defined actions, subparsers, and dynamically assembled options are outside this grammar.

## Synchronous subprocesses

The runtime supports a sealed `subprocess.run` boundary with exact `list[str]` arguments. Capture requires literal `capture_output=True` and either `text=True` or `encoding="utf-8"`. Both forms decode strict UTF-8. `check` is a boolean expression, defaulting to false. `cwd` accepts a string, native `Path`, or a nullable form of either, and `env` accepts `dict[str, str]` or `None`. Arguments may be positional or named `args`; the other parameters are keyword-only. All supplied expressions evaluate once in source order before launching the child, even when keyword order differs from parameter order.

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

The call waits for the child, captures stdout and stderr independently, and inherits stdin. It closes other inherited descriptors in the child and restores the default pipe and file-size signal dispositions. Executable lookup respects the supplied environment and child working directory. Arguments pass literally to the executable; shell expansion is never implicit. Both completed captures use strict UTF-8 and translate CRLF and bare CR to LF. Decoding happens after the child has been reaped and before a nonzero status is checked.

The result has the exact type `subprocess.CompletedProcess[str]`; bare `CompletedProcess` denotes the same sealed type. Its read-only fields are `args: list[str]`, `returncode: int`, `stdout: str`, and `stderr: str`. Empty captures are empty strings, and signal termination produces a negative return code. Results can be stored in classes, containers, and optionals. Equality compares identity; `str`, `repr`, formatted output, and `isinstance` support the sealed result. `args` retains the original argument list, so later list mutation remains visible through that field. Calling `check_returncode()` immediately checks the stored status; escaping that bound method is unsupported.

`check=True` or a failing `check_returncode()` raises the catch-only `subprocess.CalledProcessError`, which matches `subprocess.SubprocessError` and `Exception`. It exposes read-only `returncode: int`, `cmd: list[str]`, and `stdout`, `output`, and `stderr` strings; `stdout` and `output` expose the same capture. `cmd` aliases the original argument list, and the diagnostic reflects its current contents. These fixed getters raise `AttributeError` on unrelated exception instances. Missing executables and invalid working directories retain their ordinary operating-system error categories; they are not subprocess status errors.

`os.environ.get(key, default)` reads one environment variable. Both arguments must be strings. `print(text, file=sys.stderr)` writes a string and a newline to standard error.

`os.environ.copy()` returns an independent `dict[str, str]` snapshot. Passing a dictionary as `env` replaces the child's environment; use a snapshot to preserve inherited variables before making overrides. `env=None` inherits the existing environment. Snapshotting undecodable Unix names or values raises `UnicodeDecodeError`; surrogateescaped strings remain outside Relar's string domain. NUL-containing arguments, directories, or environment entries, and environment keys containing `=`, raise `ValueError`. Relar accepts an empty environment name through `execve`, as does CPython's fork/exec path. CPython's `posix_spawn` path raises `ValueError` for that name.

The optional `timeout` keyword accepts signed integer literals or expressions of exact type `float | None` in seconds. `None` waits without a deadline. A deadline expiry kills and reaps the direct child, closes the capture pipes, and raises catch-only `subprocess.TimeoutExpired`, which matches `SubprocessError` and `Exception`. Its read-only fields include `cmd: list[str]` and `stdout`, `output`, and `stderr: bytes | None`. The exception retains the original integer or float for its message and `.timeout` field. A program can read `.timeout` when all its deadline sites agree on a numeric type. Partial captures remain bytes even in text mode, so incomplete UTF-8 is preserved. During pipe capture, an observed empty stream is `b""`; a stream not observed before the deadline is `None`. If both pipes close and waiting for the child itself expires, all capture fields are `None`, even after earlier reads, matching CPython. The runtime does not wait indefinitely for descendants holding inherited pipes. This API terminates the direct child and does not manage a process group.

An `except subprocess.TimeoutExpired as error` binding has these exact field types. Stored generic exception values can be narrowed with `isinstance(error, subprocess.TimeoutExpired)` before accessing timeout fields. `cmd` retains the original list and `stdout` aliases `output`. Launch precedes timeout validation, so a missing executable takes precedence over an invalid timeout. Zero and negative timeouts, including negative infinity, launch the child and expire immediately; NaN raises `ValueError`, while positive infinity or an unrepresentable polling duration raises `OverflowError`. Exceptions can be retained and re-raised through the existing sealed exception storage; constructing or annotating a `TimeoutExpired` value directly is unsupported.

Other subprocess modes are rejected, including `shell`, `Popen`, input data, stream redirection, descriptor options, arbitrary encodings, and custom path or mapping protocols. Result construction, field replacement, exception construction, inheritance, and general access or mutation through `os.environ` are unsupported. This API adds process execution without embedding a Python interpreter or exposing an asynchronous process handle.

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

Relar commits captures only after the complete pattern succeeds, so a failed pattern leaves existing bindings unchanged. Python deliberately leaves partial bindings from failed patterns unspecified; programs intended to run under both implementations must not observe them. This does not apply to failed guards, whose successful pattern bindings are retained in both implementations.

Each capture has one concrete storage type, following ordinary assignment rules. OR alternatives must bind the same names with the same type, or types that join as `T | None`. Duplicate captures, unreachable alternatives after an irrefutable pattern, and an unguarded irrefutable case before the final case are rejected. A local used after a match must be assigned on every path reaching that use. A successful non-None pattern narrows an optional subject; a failed `None` pattern permits later cases to use its payload. Only a still-unmodified local subject receives those facts.

A match on `tuple[bool, bool]` is exhaustive when unguarded literal pairs cover all four combinations. OR patterns may group pairs. For `match (left, right)` on two locals, a final wildcard can contain `assert_never((left, right))` as its sole statement once the four pairs have been covered. The assertion must repeat both locals in order, and a capture or guard that rebinds either local invalidates the proof.

Sequence patterns require concrete lists or fixed tuples, including an optional subject after excluding `None`. Tuple patterns cannot request more fixed positions than the declared tuple contains, and a starred tuple rest must fit one concrete list element type; `T` and `None` can form `list[T | None]`. An empty tuple rest uses an existing capture's list annotation when available and otherwise defaults to `list[int]`. Strings and user-defined sequence protocols are excluded. Mapping patterns require a concrete dictionary and literal keys. A missing key fails the pattern even when the value type is optional; a present key containing `None` remains distinguishable. Extra entries are permitted, and `{}` matches any dictionary.

Numeric literal patterns use exact Python value equality across integers, floats, and booleans, so `case 1.0` matches both `1` and `True` without changing the captured value's type. Huge integers are never rounded to floats for comparison. Singleton patterns use identity, so `case True` does not match `1` or `1.0`. Bytes literals compare bytes exactly and do not match strings.

Mapping literal keys use the dictionary's scalar equality, including `True` and `1.0` matching an integer key `1`. Finite integral float keys are normalized to their exact integer value; fractional and infinite float keys cannot match integer dictionaries and fail the pattern without a conversion error. Ordinary dictionary writes still require the exact declared key type. Repeated equal literal keys are rejected, including numeric aliases, signed zero, bytes, and repeated fractional or infinite floats, even for a dictionary with a different key domain.

Class patterns resolve imported aliases to a fixed nominal identity and inspect declared fields in keyword order. Another declared class does not match even if its fields have the same names. Positional class patterns, `__match_args__`, builtin class patterns, undeclared fields, dynamic value-pattern lookups, inheritance, metaclasses, and property lookups in patterns remain unsupported. All case bodies and guards are checked, including unreachable cases.

## String enums and literal strings

`enum.StrEnum` declares a closed set of string-valued members. Use a string literal, a previous member, or `auto()` on an ASCII member name. Imports and re-exports preserve the class identity. The first member with a given value supplies its canonical `.name`. Relar stores a member as an integer tag and shares the strings behind `.name` and `.value`.

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

Use `typing.assert_never(member)` as a statement to check that every alternative of a local string enum has been handled. A nullable enum also requires handling `None`. Relar asks ty for the argument’s flow type at that line and accepts the call when it is `Never`. Adding a member without handling it makes the program fail to compile, including when the call carries a type-ignore comment. Calls inside a `finally` suite are rejected because that suite can also run during a return, break, or exception. For a member declared with `auto()` and then aliased, ty can require guards for both member names to prove exhaustion.

`Literal["added", "removed"]` accepts exactly the listed strings in parameters, returns, defaults, fields, and typed containers. A smaller set of allowed values can flow to a larger one or to `str`. A direct equality check against a fixed string can narrow a plain string local. Reassigning the local discards that proof. These annotations retain ordinary string storage and operations. Mutable containers keep their declared element types to protect writes through aliases.

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

Ordinary class bodies contain field annotations without defaults, typed methods, and optional docstrings. An instance method's first positional parameter is named `self`; its exact class type is inferred, or an explicit annotation must name that class. Other parameter and return types follow the same inference rules as direct functions; an initializer must return `None`. Method calls use the receiver's exact class and a fixed implementation. Passing or returning an instance preserves its identity, and aliases observe field replacement. Ordinary classes use identity for `==` and `!=`; distinct nominal classes compare unequal.

An ordinary class requires `__init__(...) -> None`, including a class with no fields. Its body contains only straight-line `self.field = expression` assignments, docstrings, and `pass`. Every declared field must be assigned exactly once. Assignment values may use constructor arguments and checked expressions, but cannot read or pass the receiver; a partially initialized object never escapes construction. Local assignments, conditionals, loops, explicit returns, and direct later calls to `__init__` are rejected.

`@property` marks a read-only getter with only `self`. Each access calls it again, preserving mutation, ownership, and exceptions. Setters and deleters are unsupported. `@classmethod` binds a first positional parameter named `cls` to the declaring class; `cls(...)` and `cls.method(...)` resolve statically. `@staticmethod` has no implicit parameter. Class/static methods work through either a class or an instance, and an instance expression still evaluates before arguments. `Class.method(instance, ...)` also supports unbound instance methods. `typing.Self`, including aliases and quoted annotations, means the owning class in method annotations; it is not a runtime type object or a stored class value.

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

Class options `init`, `eq`, `repr`, `frozen`, `kw_only`, and `slots` are supported. A mutable dataclass may retain an explicit initializer under the ordinary constructor rules; `init=False` requires one. Both values of `slots` keep the language's fixed-layout contract. Frozen records reject field assignments at compile time; their referenced containers remain mutable. The generated initializer calls an instance method `__post_init__(self) -> None` after assigning every field and default factory. An explicit initializer controls its own calls. Explicit frozen initializers, `InitVar`, `ClassVar`, `field(init=False)`, custom metadata, generated ordering, custom hashes, and weak-reference slots are unsupported. `match_args` is accepted without adding positional class patterns or runtime `__match_args__` access. Frozen records do not add class-valued dictionary keys or set members.

Generated equality follows modern CPython: an object equals itself; otherwise instances of the same dataclass compare selected fields in declaration order. `eq=False` preserves identity equality. Distinct records containing the same scalar NaN compare unequal. Compared container fields containing floats are rejected because native unboxed floats cannot preserve Python container NaN identity; use `field(compare=False)` or `@dataclass(eq=False)` for such storage. Nested record fields preserve their own identity and equality behavior.

Fixed classes and dataclasses can define `__str__(self) -> str`. `str(record)`, `print(record)`, `f"{record}"`, and `f"{record!s}"` call this method once. Without an override they use the record’s repr. The method also supports ordinary instance calls and bound method values.

Generated dataclass repr includes field names and values in declaration order, omitting `field(repr=False)`. `repr(record)`, `f"{record!r}"`, and representations of records inside containers use this repr even when the record defines `__str__`. With `repr=False`, the class uses the ordinary object repr, which includes its address. Resource fields require `repr=False` on the field or class.

Fixed classes reserve field names beginning with `__`. Their supported special methods are `__init__` and `__str__`. Inheritance, metaclasses, class variables, and dynamically added fields remain unsupported. Class-field ownership types must form an acyclic graph, following list elements, dictionary values, tuple positions, and optional payloads. For example, fields `children: list["Node"]`, `named: dict[str, "Node"]`, and `parent: "Node | None"` inside class `Node` are rejected even when unused or initialized empty. Methods may accept or return their own class because those signatures do not create stored references.

## Concrete containers and optional values

`list[T]` has one concrete element type, such as `list[str]`, `list[Counter]`, or `list[list[int]]`. Types are invariant: an existing `list[str]` cannot become `list[str | None]` through assignment or a call. Every append and indexed write checks the exact element type. Repetition copies references to the same elements, so `[row] * 3` shares `row` across all three positions. `list[int]` retains its specialized tagged-integer storage and runtime calls; other lists use owned runtime slots.

An unquoted `Sequence[T]` parameter from `collections.abc` or `typing` accepts
an exact `list[T]` or an omitted `()` default. Its body may iterate the
parameter, apply `map(str, parameter)`, or call `len`. Parameters without a
tuple default also support read-only indexing and slicing.
It shares the caller’s list, so mutations through existing aliases remain visible.
A function returning `Sequence[T]` can return an exact `list[T]`.

`dict[K, V]` accepts exact `int`, `str`, `bytes`, `Path`, or immutable fixed-tuple keys and one concrete value type. Tuple keys may recursively contain `int`, `bool`, `str`, `bytes`, `Path`, `None`, and nullable forms of those immutable fields. Floats, records, mutable containers, and the `str | bytes` union are excluded from keys. Keys use value equality under one exact type; booleans cannot stand in for integer keys, and a bare `bool` is not a key type. Nonempty literals infer these types from their first entry; `{}` and `dict()` require a concrete contextual type. Duplicate keys keep the first insertion position and the last value. Replacing a value keeps its position, while deletion and reinsertion move the key to the end. `mapping[key]`, assignment, `del mapping[key]`, integer augmented assignment, `len`, and single `in`/`not in` comparisons are supported. Missing required reads and deletes report `KeyError`.

`mapping.get(key)` returns `V | None`, flattened when `V` is already optional or `None`. A default must fit `V | None`; a guaranteed `V` default gives a `V` result. Defaults are evaluated eagerly even when the key exists. Container literals in defaults receive the dictionary's declared value type. Dictionaries are invariant in their key and value types.

`collections.defaultdict[K, list[T]]` accepts `defaultdict(list)` in a typed context. A missing subscription inserts a fresh list and returns it. Further subscriptions return that same list through every alias, including a `dict[K, list[T]]` parameter. Membership, `get`, and key unions leave missing keys absent.

Direct `mapping.keys()` operands and sets of the same key type support `|`, including chained unions. Each union reads the two key sources after evaluating both operands, and returns an independent set. Stored key-view objects and arbitrary iterables as union operands are outside the supported subset.

`set[T]` accepts the same exact scalar or immutable fixed-tuple shapes as dictionary keys, with the same value equality. Nonempty literals infer their element type and evaluate every element from left to right, including duplicates. Empty `set()` requires a concrete contextual type. `add(value)` inserts an element, `discard(value)` removes it when present, and both return `None`. Adding a duplicate or discarding a missing element is a no-op. `len`, single `in`/`not in` comparisons, direct iteration, and loop-head `enumerate` are supported. Set iteration order is unspecified and programs must not depend on dictionary-like insertion order. Iteration checks size changes and owns each yielded element before the body can remove it. Copy/iterable construction, intersection, difference, symmetric difference, comparisons, `remove`, `pop`, and `clear` are not implemented.

Tuple types name each position, for example `tuple[str, int, list[Counter]]`; `tuple[()]` is empty. A tuple's slots cannot be reassigned, but a referenced list or instance can still be mutated. Indexes must be in-range integer literals, optionally negative. Variable indexes, computed indexes, variadic tuple annotations, tuple concatenation, and tuple repetition are rejected. Tuple equality and inequality require the same concrete immutable shape accepted for dictionary keys. Ordered scalar tuple shapes additionally support lexicographic `<`, `<=`, `>`, and `>=`, including nested tuples and fields whose exact type is `None`. Nullable tuple fields permit equality but exclude ordering. Lists of matching immutable scalar tuples support `==` and `!=`. They compare lengths and then tuple values in order. Mutable tuple fields and float fields are outside this comparison.

Fixed tuples, lists, strings, and bytes support the nested and starred assignment patterns described in [assignment and container expansion](#assignment-and-container-expansion). Concrete list lengths are checked at runtime; fixed-tuple shapes are checked during compilation.

`for value in values` evaluates a concrete list once and retains that original list even if the source name or field is later replaced. It reads the current length and acquires the next element at each step. Appending, removing, or replacing elements through an alias therefore has Python's index-based list-iteration behavior. The current item remains owned if the body removes it from the list. `enumerate(values, start)` captures the list before evaluating its optional integer start and yields fixed `(int, T)` tuples; its counter supports arbitrary precision and is independent of visible target rebinding. Both forms support `break`, `continue`, nested loops, and early returns. A target first introduced by a loop remains potentially uninitialized afterward because the loop may be empty. Escaping iterator values and user-defined iteration protocols are not supported.

`for` and `while` support an `else` suite. It runs when iteration is exhausted or the condition becomes false, including zero iterations. `break` skips it; `continue` does not. Returns and escaping errors skip it. A `finally` block can override any pending transfer: a break replaced by continue can eventually reach else, while a continue replaced by break skips it. The completed loop is no longer an enclosing loop for its else suite, so `break` and `continue` there refer to an outer loop. Compiler-owned iterator and source roots are released before the else suite, preserving drop-time file flushing; the visible loop target keeps its usual last-item binding.

The checker joins assignments from surviving break paths with the normally completed else suite. A search can assign its result before every break and assign a fallback in else, then use the result afterward. An else suite does not make the iteration target initialized when the iterable was empty. False-condition optional facts are available inside a while-else suite; finally writes and exception-handler target deletion cannot preserve stale facts on break paths.

`for key in mapping` and `for key, value in mapping.items()` visit insertion order. `enumerate` accepts either form and creates the iterator after evaluating its start argument. Each yielded key/value has its own owner before the body runs. Replacing an existing value is allowed and later iterations see the replacement. A changed size at the next step reports `RuntimeError`, including after the last yielded item. Same-size structural mutation does not provide a portable traversal guarantee; it may report changed keys or skip entries. `items()` is supported only at a loop boundary, including inside loop-head `enumerate`; views and iterators cannot be stored as user values.

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

Capturing an outer local preserves its value and shared object identity; field and indexed reads still occur where written inside the body. Mutations through aliases retain the existing list/dictionary/set iteration behavior. A failure propagates through normal handlers and finally blocks, releasing the partial result and temporary owners without rolling back earlier user-visible mutations. Eager comprehensions do not expose an iterator, closure, callback, or partially built result. Generator expressions use the separate lazy rules above; generator functions, async clauses, and user-defined iteration protocols remain unsupported.

## Ownership and errors

Assignment and return expressions produce owned references when their type needs one. Operator and call operands may borrow a direct local only when later expression evaluation cannot replace that binding; earlier operands retain separate owners across assignment expressions. Parameter bindings that are never reassigned borrow their caller's root; other local slots own their values and release them on exit. A return acquires or transfers ownership even when returning a borrowed parameter. List-element reads generally remain owned across later evaluation; field and tuple reads, optional unwrapping, pops, and computed results are also owned, so mutation cannot invalidate an intermediate value. This also keeps a method receiver alive if later arguments replace its original field. An assignment publishes the replacement before dropping the old value. Ordinary subscript and field assignment evaluate the RHS first; augmented assignment instead captures the target and old value before evaluating its RHS.

Integer representation is canonical: values from `-2**62` through `2**62 - 1` always use tagged machine words; values outside that range use `num-bigint`. Checked arithmetic promotes and demotes as needed. Foreign adapters must preserve this invariant before passing integers to generated code.

Conservative interval facts track immutable integer values in local slots across reassignment and control flow. Heap reads and user-call results start unknown. Direct arithmetic requires proofs that both operands and the result fit the small representation; only proved representable arithmetic and tagging shifts receive LLVM's `nsw` overflow flag. Unknown bounds, analysis overflow, or a possibly zero divisor retain checked lowering. Division by zero remains a language error at every optimization level. `range` arguments are captured once, and a specialized hidden cursor must also fit after its final increment; assigning to the visible target does not change its progression.

A whole-function proof can omit old-value and exit ownership checks for an integer local when every possible write or move into that slot is proved small. Parameters and unknown replacements disqualify that whole-slot proof. A separate assignment-time proof can omit old-value cleanup after the RHS succeeds when its first operand reads that exact slot as a proved small integer and later evaluation cannot replace, clear, or move the slot. Exit and exceptional cleanup retain their own ownership checks.

A proved nonnegative small index may use a decoded native-index ABI. Ordinary reads and all writes still check the container's current bounds. A separately guarded canonical reduction can prove an exact source/cursor read in bounds before its unique cursor advance, while the source remains rooted and unchanged. Such reads use an infallible borrowed accessor; reads after the advance, in the loop's else suite, at another index, or from another source retain checks. Unknown calls, mutation, hidden writes, and early continue paths must satisfy the proof or keep the original checked loop. Negative or arbitrary-precision indices retain their normal normalization and error behavior.

Element reads otherwise acquire ownership before later evaluation can remove them. A direct integer-list read in a two-operand integer comparison can also borrow when both complete operand trees are proved nonmutating; the local keeps the list rooted until the comparison consumes the handle. Chained comparisons and potentially mutating evaluation retain owned reads. A same-list copy with simple, proved indices can combine its get and set, checking and acquiring the source before checking and replacing the destination.

Heap objects are nonmoving reference-counted allocations. Classes and tuples use concrete fixed layouts, and homogeneous containers store their representation metadata once per container rather than in each element. Optionals occupy one nullable word: zero means absent, present integers and pointers keep their payload word, and optional booleans encode False/True as one/two. Present optional floats own a separate immutable binary64 allocation so every float bit pattern remains available; ordinary floats remain unboxed. These representations do not change the supported Python values or shared mutation behavior.

Writes acquire the replacement before releasing the displaced value, and reads acquire ownership before the receiver can be released, apart from the explicitly proved rooted integer borrows above. Fixed records start with zeroed fields and use a static destruction descriptor, so failed construction safely releases every initialized owner. Generated field accesses are direct; their receiver remains rooted and the checker proves complete initialization before a read. Container storage uses scoped `UnsafeCell` closures on one execution thread: mutable access is exclusive, operations cannot reenter through aliases, and no storage reference or resizable-buffer address may escape. Independently owned values and explicitly constrained borrowed integer handles may escape; displaced owners are dropped after access ends.

Dictionaries use a randomized hash index and insertion-ordered entries. Integer, string, and byte keys have specialized storage; sets own keys without dummy value slots or an ordering guarantee. Internal iterators root their container and prevent entry compaction until released, but keep no storage reference across a loop body. The acyclic heap type graph prevents reference cycles through classes and containers. There are no finalizers or user callbacks under storage access. Threaded or reentrant extension boundaries would need to preserve these rules. This is not yet a general garbage collector, extension ABI, sandbox, or proof of memory safety; those claims would require additional design and evidence.

Language errors propagate through explicit checked call results and can be caught. Each fallible runtime call records an owned pending exception and reports failure through either its result or a pending-state query; generated code checks before decoding or acquiring a result. Integer-list reads reserve raw zero for failure, while writes return a 32-bit success flag. Other calls retain the pending-state check. Proven fixed-field operations and functions whose complete bodies cannot raise omit redundant queries; their argument and receiver evaluations retain their own checks. Partial expression values, constructor fields, unpacking temporaries, and loop iterator roots are released when their scopes exit. Function propagation also releases owned locals. No Rust panic or native stack unwinding implements language exceptions. Uncaught errors print an exception category and message and return process status 1 after cleanup. Actual allocation failures and invalid internal ABI operations may still terminate the process. The compiler and runtime archive must be rebuilt together when this private ABI changes.

Exceptions contain an immutable category, message, optional explicit cause, and context-suppression flag. Process status errors additionally retain the original argument list, return code, and captured strings, and format their message from the current argument list. A cause can be attached only while raising a freshly constructed builtin exception, and cause attributes cannot be written, so cause chains remain acyclic. Deep uniquely owned cause chains are released iteratively. There are no traceback, frame, or implicit context references. Exited function frames release their owners during propagation; implicit file closure can therefore happen before CPython releases locals retained by a traceback. Full traceback objects, implicit exception context, attaching causes to existing instances, custom exception subclasses, exception groups, `except*`, and user-defined context managers remain unsupported.

## Exceptions and cleanup

The constructible builtin categories are `Exception`, `ArithmeticError`, `ZeroDivisionError`, `OverflowError`, `LookupError`, `IndexError`, `KeyError`, `ValueError`, `AssertionError`, `RuntimeError`, `EOFError`, `OSError`, `TypeError`, `AttributeError`, `MemoryError`, `SystemError`, `UnicodeError`, `FileNotFoundError`, `PermissionError`, `IsADirectoryError`, and `NotADirectoryError`, plus the imported `io.UnsupportedOperation`. A constructor takes zero arguments or one string message. `raise Category` constructs the zero-argument form; `raise value` requires an exception instance. The annotation `Exception` admits this sealed family and can be used in parameters, returns, fields, containers, and optional values. Specific subclass annotations are currently rejected rather than erasing their narrower contracts. `BaseException` and `UnicodeDecodeError` are supported as handler categories, but not as constructors or storage annotations.

Handlers select the first matching fixed builtin category or nonempty tuple of categories, using the builtin hierarchy. A bare handler catches any supported exception. `except Category as name` binds an owned exception and deletes that name on every handler exit, even if it previously existed or was rebound inside the handler. Save it under another name to retain it. Bare `raise` uses the dynamically active exception, including from a called function or a `finally` block unwinding an exception; without an active exception it raises `RuntimeError`.

`raise ValueError("message") from cause` and `raise ValueError from cause` accept an exception, supported builtin exception class, nullable exception, or `None` as the cause. The raised exception evaluates before the cause. Readonly `error.__cause__` and `error.__suppress_context__` expose the resulting metadata; `from None` sets suppression with an absent cause. Ordinary re-raising preserves existing metadata. Uncaught explicit chains print their categories and messages in cause order, without traceback objects.

`else` runs only after normal fallthrough from the protected body; its errors are handled outside that try statement. `finally` runs on normal fallthrough, error propagation, `return`, `break`, and `continue`. A new transfer from finally overrides the pending transfer and releases its owned return value or exception. Loop transfers inside finally use the block's lexical loop. Nested handlers restore the previously active exception.

Handlers cannot assume a fallible assignment completed. Definite assignment starts from the protected body's incoming state, excluding names that a nested handler could delete. Optional facts for potentially reassigned names are discarded. Finally is checked against a similarly conservative entry covering all exit reasons; initialize values before try when finally needs them. Integer interval facts in handlers and finally start unknown, and facts after try are conservative. These restrictions can reject safe programs requiring more precise exceptional-flow analysis.

`str(exception)` and `print(exception)` expose the message, including Python's special quoting for a one-string `KeyError`. Missing dictionary keys report the actual integer key or quoted string key. KeyError quoting follows Ruff's repr rules and pinned ICU Unicode data; newly assigned characters can print literally where a CPython build with older Unicode data emits an escape (for example U+1FAEA versus CPython 3.14). Other runtime-generated diagnostic messages are not a CPython text-compatibility promise; category matching and explicitly constructed messages are stable within this quoting rule. `assert` is always enabled independently of optimization, evaluates its optional string message only when false, and raises an empty-message `AssertionError` when no message is provided.

Standard output uses bounded buffering for pipes and files, and flushes each printed line on a terminal. Pending output is also flushed before `input()` waits, before a reported runtime error exits, and before the generated process entry returns normally. If a write or flush returns an error, Relar reports `OSError`; fatal-error flushing is best effort and preserves the original exception category. Generated code explicitly flushes at process exit because its LLVM entry point does not run Rust's standard `main` lifecycle.

Before creating file writers or initializing standard output, the runtime ignores `SIGPIPE` once so disconnected pipes produce catchable I/O errors. The sealed runtime has no user signal handlers or signal API.

## Link options

`relar build` and `relar run` remove unreachable code and data at every optimization level.
Relar passes `-dead_strip` to the macOS linker and `--gc-sections` to the Linux linker.
Use `--no-gc-sections` to keep unused code. On Linux, Relar also enables full RELRO so the
loader can make the global offset table read-only after resolving dynamic symbols at startup.
Select `-C relro-level=partial` for lazy binding or `-C relro-level=off` to disable RELRO.

Pass `-C link-arg=ARG` once for each extra argument to Clang, in command-line order.
Use Clang's `-Wl,` syntax for linker options. For example, this Linux build writes a link map
and prints the Clang invocation to standard error.

```sh
relar build totals.py -C link-arg=-Wl,-Map=totals.map --verbose-link
```

Set `RELARFLAGS` to whitespace-separated build options. For arguments containing spaces,
set `RELAR_ENCODED_FLAGS` to UTF-8 arguments separated by ASCII unit separators (`0x1f`).
When set, the encoded variable replaces `RELARFLAGS`, even when empty. Relar reads these
options before the command line. The last value wins for individual settings, and
repeated linker arguments accumulate in order. For example, this command enables section
GC for one build.

```sh
RELARFLAGS='--no-gc-sections' relar build totals.py --gc-sections
```

These variables apply to `build` and `run`. Use `--` on the `run` command line to pass
arguments to the compiled program.

## Incremental builds

`relar build` and `relar run` share a per-user cache on native x86-64 Debian/Ubuntu
with distribution Clang 18, GNU BFD, and an ordinary runtime archive. Both commands
reuse the last executable when its sources, import lookups, build options, compiler,
runtime, and native link inputs are still current. Program arguments and the output
path can change between invocations.

After a source edit, Relar checks and lowers the entire import graph. It recompiles
the modules whose emitted LLVM changed and relinks the program. Changing a public
signature or a shared layout can also recompile a caller. Changing only section GC
or RELRO relinks with the existing objects. `--verbose-link` performs the link and
prints its actual command, including when all the module objects are cached.

Build status goes to stderr. Use `-v` or `--verbose` to see each compiled and
reused module, or `-q` or `--quiet` to suppress progress. Set `NO_COLOR` for plain
terminal output. `--whole-program` optimizes all modules together and uses a
separate executable cache. `--no-cache` bypasses cache reads and writes while
preserving the selected code generation mode. These options also work in
`RELARFLAGS` and `RELAR_ENCODED_FLAGS`.

Custom Clang drivers and raw `-C link-arg` arguments, explicit targets, emitted
LLVM, sanitizers, ThinLTO, and unsupported toolchains use the whole-program
backend without caching. By default, cache data lives under
`$XDG_CACHE_HOME/relar/v1` or `~/.cache/relar/v1`. An absolute `RELAR_CACHE_DIR`
overrides the parent directory. Older objects stay available for reverts until
that cache directory is cleared.

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
python3 tests/dataclass_records.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/containers.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/comprehensions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/unpacking.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/fstrings.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/calls.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/inference.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/iterables.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/luhn.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/expressions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/exceptions.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/files.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/processes.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/paths.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/toml_records.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
python3 tests/io.py --compiler target/release/relar --runtime target/release/librelar_runtime.a
```

Build the compiler and runtime archive from the same revision. Relar finds the archive beside the compiler executable, or accepts an explicit path with `--runtime`. `--clang` selects the Clang driver and `--opt-level` selects 0 through 3 (default 3). Every input module, executable, runtime, explicit ThinLTO linker, and saved LLVM output must have a distinct file identity; symlinks and hard links cannot bypass the input overwrite check.

`--lto-linker PATH` enables optional cross-language ThinLTO and requires explicit `--runtime` and `--target TRIPLE`. Build that runtime with `RUSTFLAGS=-Clinker-plugin-lto` in a separate target directory, and select an LLVM linker that can read both the Rust and Clang bitcode. On Linux, use Clang 18 with the pinned Rust toolchain's bundled LLD. At optimization level 3, Relar raises LLVM's import and inlining budgets to 1,000 so runtime accessors can inline across the language boundary; this trades additional code size and link work for fewer runtime calls. Other optimization levels retain LLVM's normal budgets. `--target` also works without ThinLTO; it does not expand the supported runtime platforms.

On Apple Silicon, build the bitcode runtime with `--target aarch64-apple-darwin` so Cargo builds host tools normally. Pass the resulting `target/lto/aarch64-apple-darwin/release/librelar_runtime.a` to `--runtime`. Use the Mach-O linker `ld64.lld` from an LLVM release that reads both the Rust bitcode and your macOS SDK.

The following example builds the dependency planner on Linux.

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

`relar build` and `relar run` use the system allocator by default. On Linux,
`--allocator jemalloc` selects the statically linked jemalloc heap for runtime
objects. `--allocator system` selects the platform heap explicitly. macOS and
`--sanitize` use the system heap. The compiler looks beside its executable
for `librelar_runtime.a` (system) or `librelar_runtime_jemalloc.a` (jemalloc).
An explicit `--runtime` selects the allocator through the archive itself.
For a Linux ThinLTO build with jemalloc, use `-p relar-runtime-jemalloc` in
the Cargo command above and pass `--runtime target/lto/release/librelar_runtime_jemalloc.a`.
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
python3 tests/workspaces.py --compiler target/release/relar --runtime target/release/librelar_runtime.a --python python3.14 --clang clang-18
```

The files suite compares complete filesystem effects, strict decoding, newline handling, write counts, alias-visible closure, all `with` exit reasons, failed cleanup, argument aliases, and descriptor recovery under a reduced open-file limit. Rejection cases protect the sealed import, file, context-manager, and argument surfaces. The existing suites also run generated-code and runtime AddressSanitizer checks at `-O1` in CI.

The bytes suite compares full byte-range repr/indexing, strict UTF-8 decoding errors, constructors, exact typed storage and keys, 896 empty/nonempty slices, iterator ownership, and failures during partial construction at `-O0` and `-O3`. Binary fixtures compare complete file contents, append/exclusive modes, imported handle aliases, closed/wrong-direction errors, and every context-manager exit path. It accepts the same target, ThinLTO, and sanitizer options as the other native suites:

```sh
python3 tests/bytes.py --compiler target/release/relar --runtime target/release/librelar_runtime.a --clang clang-18
```
