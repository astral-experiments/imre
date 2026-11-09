# Compiler design

Imre compiles Python source to native code and links the executable with a Rust runtime. It has two ways to compile a program. When it can resolve the types and calls in a module graph, it specializes operations on those types. Programs that need runtime lookup can use boxed Python objects. Both paths emit LLVM IR. [Compiler entry points](../crates/imre-compiler/src/lib.rs)

## Typed compilation

Ruff parses the source. Imre resolves imports, function signatures and class layouts across the graph, and uses ty for semantic queries and diagnostics. The frontend lowers supported operations into typed IR. A call records its arguments in Python evaluation order and separately records the order the callee receives them. This matters when two keyword arguments have side effects. [Frontend](../crates/imre-compiler/src/frontend.rs) · [IR](../crates/imre-compiler/src/ir.rs) · [Supported Python](subset.md)

Specialization runs before LLVM emission. For example, range analysis can prove that an integer operation fits in a machine word, and borrow analysis can prove that a callee preserves a list reference. Without those proofs the compiler emits checked arithmetic and retains the references it needs. Debug builds and builds with `verify-ir` check the IR after each specialization pass. [Range analysis](../crates/imre-compiler/src/ranges.rs) · [Borrow analysis](../crates/imre-compiler/src/borrows.rs) · [IR verifier](../crates/imre-compiler/src/verify.rs)

The compiler checks and specializes the whole graph before it splits the LLVM output by source module. An edit can then reuse an unchanged module’s native object. The in-process incremental compiler also retains ty’s Salsa database across checks. [Code generation units](../crates/imre-compiler/src/llvm/units.rs) · [Build cache](../crates/imre-compiler/src/build_cache.rs)

## Boxed compilation

The boxed emitter compiles branches, loops and function bodies to LLVM and calls the runtime for operations such as attribute lookup and calls through a Python object. It uses handles into a runtime-managed heap. This lets the same operation invoke different Python methods depending on its operands. [Boxed emitter](../crates/imre-compiler/src/dynamic/emit.rs) · [Object runtime](../crates/imre-runtime/src/dynamic/mod.rs)

## What an ordinary build compiles

Imre compiles executable Python bodies ahead of time. Runtime data can still drive object and method dispatch. A caller can select a handler from a dictionary with `rules[key](value)`, or rebind an attribute to a compiled function with `setattr(obj, name, double)` even when `name` comes from input.

Modules still initialize on first import, as `selectors` does with its compiled Python initializer. When `json` imports its helpers, the loader passes their linked native code objects to `exec`. [Built-in initializers](../crates/imre-compiler/src/dynamic/mod.rs#L2170) · [Import loader](../crates/imre-compiler/stdlib/importlib/_bootstrap_external.py#L129)

The compiler reports a reachable `exec(input())` or `runpy.run_path(sys.argv[1])` before linking because it cannot identify the Python body. For a fixed set of plug-ins, declare their modules at build time and select a compiled handler at runtime. A handler that uses `runpy.run_path` can pass the literal filename of a module declared with `--include-module`. [Executable source](../crates/imre-compiler/src/analysis/execution.rs) · [Executable files](../crates/imre-compiler/src/analysis/run_path.rs)

Ordinary builds check arguments against explicitly annotated parameters of first-party functions even when an unrelated operation selects boxed compilation. The typed path rejects the callable dictionary above. [Argument-checking tests](../crates/imre-compiler/src/analysis/closed_program_tests.rs) · [Callable types](subset.md#callable-values-and-callbacks)

A Python-constructed `SourceFileLoader` can execute a physical module’s linked initializer after the file changes or is removed when given the registered name and exact filename as a plain string. With a path-like object, the standard loader reads the file first and can use the initializer if the read and conversion yield that same filename. Custom read and conversion hooks still run. When a selected compiler calls `compile(data, filename, "exec")`, matching registered bytes, origin and compiler settings can select the linked initializer. A code object created for inspection can expose its filename and constants. Executing it requires a compiled body. Issued loaders for bundled modules retain access to their own virtual origins.

## Runtime

On the typed path, small integers fit in a tagged word and larger integers use a reference-counted `BigInt`. Containers own their elements. Reading an element before a callback that can remove it requires keeping the value alive across that call. Runtime functions and the emitter agree on which arguments they borrow, which results they transfer, and how they signal a Python exception. [Runtime calling conventions](../crates/imre-runtime/src/lib.rs) · [Compiler-side contracts](../crates/imre-compiler/src/contracts.rs)

Native extension modules link through Imre’s supported `Python.h` interface. Their wheels carry both a static archive and typed signatures for the compiler. [Building extensions](extensions.md)
