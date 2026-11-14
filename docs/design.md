# Compiler design

Imre lowers supported Python bodies to checked intermediate representation (IR), emits LLVM, and links the executable with a Rust runtime. The same function can perform native arithmetic and call a Python object selected at runtime. [Compiler entry points](../crates/imre-compiler/src/lib.rs) · [Operation emitter](../crates/imre-compiler/src/llvm/operations.rs)

## Checking and specialization

Ruff parses the source. Imre resolves imports, function signatures and class layouts across the graph, and uses ty for semantic queries and diagnostics. The frontend lowers supported operations into typed IR. A call records its arguments in Python evaluation order and separately records the order the callee receives them. This matters when two keyword arguments have side effects. [Frontend](../crates/imre-compiler/src/frontend.rs) · [IR](../crates/imre-compiler/src/ir.rs) · [Supported Python](subset.md)

Specialization runs before LLVM emission. For example, range analysis can prove that an integer operation fits in a machine word, and borrow analysis can prove that a callee preserves a list reference. Without those proofs the compiler emits checked arithmetic and retains the references it needs. Debug builds and builds with `verify-ir` check the IR after each specialization pass. [Range analysis](../crates/imre-compiler/src/ranges.rs) · [Borrow analysis](../crates/imre-compiler/src/borrows.rs) · [IR verifier](../crates/imre-compiler/src/verify.rs)

The compiler checks and specializes the whole graph before it splits the LLVM output by source module. An edit can then reuse an unchanged module’s native object. The in-process incremental compiler also retains ty’s Salsa database across checks. [Code generation units](../crates/imre-compiler/src/llvm/units.rs) · [Build cache](../crates/imre-compiler/src/build_cache.rs)

## Emitting operations

The LLVM emitter walks the checked statements and expressions of a function. Its calling convention determines where arguments and locals live. The representation of each operand determines which instructions implement the operation. Consider a callback chosen from a dictionary at runtime.

```python
def publish(a: int, b: int, send):
    total = a + b
    send(total)
```

With small, exact integers, the addition can use machine arithmetic. The call to `send` looks up and invokes the selected callable through the Python runtime. Both operations belong to the compiled `publish` body. [Checked operations](../crates/imre-compiler/src/llvm/operations.rs) · [Python calling convention](../crates/imre-compiler/src/llvm/registered/abi.rs)

The driver still chooses an entry layout for the module graph. When the entry needs Python module initialization or object lookup, it can register Python functions and retain checked bodies for eligible fixed-signature callees. Those functions share the statement and expression emitter. [Entry selection](../crates/imre-compiler/src/lib.rs) · [Module and callable preparation](../crates/imre-compiler/src/dynamic/emit.rs)

## What an ordinary build compiles

Imre compiles executable Python bodies ahead of time. Runtime data can still drive object and method dispatch. For example, given `handlers = {"double": double, "triple": triple}`, the call `handlers[key](value)` selects a compiled function at runtime. Python evaluates the dictionary lookup before it evaluates the argument.

Modules still initialize on first import, as `selectors` does with its compiled Python initializer. When `json` imports its helpers, the loader passes their linked native code objects to `exec`. [Built-in initializers](../crates/imre-compiler/src/dynamic/mod.rs#L2170) · [Import loader](../crates/imre-compiler/stdlib/importlib/_bootstrap_external.py#L129)

The compiler reports a reachable `exec(input())` or `runpy.run_path(sys.argv[1])` before linking because it cannot identify the Python body. For a fixed set of plug-ins, declare their modules at build time and select a compiled handler at runtime. A handler that uses `runpy.run_path` can pass the literal filename of a module declared with `--include-module`. [Executable source](../crates/imre-compiler/src/analysis/execution.rs) · [Executable files](../crates/imre-compiler/src/analysis/run_path.rs)

For a resolved call to a first-party function, ordinary builds check explicitly annotated parameters. Passing a `str` where the callee expects an `int` produces a build diagnostic even when another module uses a runtime-selected callable. [Argument-checking tests](../crates/imre-compiler/src/analysis/closed_program_tests.rs) · [Callable types](subset.md#callable-values-and-callbacks)

A Python-constructed `SourceFileLoader` can execute a physical module’s linked initializer after the file changes or is removed when given the registered name and exact filename as a plain string. With a path-like object, the standard loader reads the file first and can use the initializer if the read and conversion yield that same filename. Custom read and conversion hooks still run. When a selected compiler calls `compile(data, filename, "exec")`, matching registered bytes, origin and compiler settings can select the linked initializer. A code object created for inspection can expose its filename and constants. Executing it requires a compiled body. Issued loaders for bundled modules retain access to their own virtual origins.

## Runtime

Small integers can fit in a tagged word and larger integers use a reference-counted `BigInt`. Containers own their elements. Reading an element before a callback that can remove it requires keeping the value alive across that call. Runtime functions and the emitter agree on which arguments they borrow, which results they transfer, and how they signal a Python exception. [Runtime calling conventions](../crates/imre-runtime/src/lib.rs) · [Compiler-side contracts](../crates/imre-compiler/src/contracts.rs)

Native extension modules link through Imre’s supported `Python.h` interface. Their wheels carry both a static archive and typed signatures for the compiler. [Building extensions](extensions.md)
