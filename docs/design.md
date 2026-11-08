# Compiler design

Imre compiles Python source to native code and links the executable with a Rust runtime. It has two ways to compile a program. When it can resolve the types and calls in a module graph, it specializes operations on those types. Programs that need runtime lookup can use boxed Python objects. Both paths emit LLVM IR. [Compiler entry points](../crates/imre-compiler/src/lib.rs)

## Typed compilation

Ruff parses the source. Imre resolves imports, function signatures and class layouts across the graph, and uses ty for semantic queries and diagnostics. The frontend lowers supported operations into typed IR. A call records its arguments in Python evaluation order and separately records the order the callee receives them. This matters when two keyword arguments have side effects. [Frontend](../crates/imre-compiler/src/frontend.rs) · [IR](../crates/imre-compiler/src/ir.rs) · [Supported Python](subset.md)

Specialization runs before LLVM emission. For example, range analysis can prove that an integer operation fits in a machine word, and borrow analysis can prove that a callee preserves a list reference. Without those proofs the compiler emits checked arithmetic and retains the references it needs. Debug builds and builds with `verify-ir` check the IR after each specialization pass. [Range analysis](../crates/imre-compiler/src/ranges.rs) · [Borrow analysis](../crates/imre-compiler/src/borrows.rs) · [IR verifier](../crates/imre-compiler/src/verify.rs)

The compiler checks and specializes the whole graph before it splits the LLVM output by source module. An edit can then reuse an unchanged module’s native object. The in-process incremental compiler also retains ty’s Salsa database across checks. [Code generation units](../crates/imre-compiler/src/llvm/units.rs) · [Build cache](../crates/imre-compiler/src/build_cache.rs)

## Boxed compilation

The boxed emitter compiles branches, loops and function bodies to LLVM and calls the runtime for operations such as attribute lookup and calls through a Python object. It uses handles into a runtime-managed heap. This lets the same operation invoke different Python methods depending on its operands. The runtime also handles source loaded during execution. [Boxed emitter](../crates/imre-compiler/src/dynamic/emit.rs) · [Object runtime](../crates/imre-runtime/src/dynamic/mod.rs)

## What an ordinary build compiles

The goal is to compile every Python body the program can execute. Runtime data can select among compiled handlers or declared module implementations. If `rules` maps two names to the functions `double` and `increment`, then `rules[rule](value)` has two possible callees even when the key comes from input.

`exec(input())` asks the running program to execute newly supplied Python source. The compiler should diagnose that operation at build time, along with namespace writes that can rebind a declared function, class or code object to an unbounded set of implementations. It should suggest a rewrite when one preserves the program’s behavior.

Today ordinary builds check arguments against explicitly annotated parameters of first-party functions even when an unrelated operation selects boxed compilation. The typed path rejects the callable dictionary above. An ordinary build can still execute `exec(input())`. Checks for general late imports and executable namespace writes are under development, and some bundled modules use the runtime evaluator. [Argument-checking tests](../crates/imre-compiler/src/analysis/closed_program_tests.rs) · [Callable types](subset.md#callable-values-and-callbacks)

## Runtime

On the typed path, small integers fit in a tagged word and larger integers use a reference-counted `BigInt`. Containers own their elements. Reading an element before a callback that can remove it requires keeping the value alive across that call. Runtime functions and the emitter agree on which arguments they borrow, which results they transfer, and how they signal a Python exception. [Runtime calling conventions](../crates/imre-runtime/src/lib.rs) · [Compiler-side contracts](../crates/imre-compiler/src/contracts.rs)

Native extension modules link through Imre’s supported `Python.h` interface. Their wheels carry both a static archive and typed signatures for the compiler. [Building extensions](extensions.md)
