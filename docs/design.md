# Relar: the first experiment

Relar compiles a closed set of first-party Python sources ahead of time through LLVM. The compiler and runtime are independent Rust implementations. The first deliverable is a useful example program, an executable that needs no Python installation, and a reproducible comparison with the same source running on CPython.

## Language contract

Types are requirements. Named functions declare parameter and return types; local types are inferred or explicitly declared. Assignments, calls, returns, and container writes must satisfy those types. A contradiction is a compilation error. Values entering from input or, eventually, foreign code are checked before a specialized representation is used. A failed check is a language error, never an unchecked memory reinterpretation.

The initial types are `int`, `bool`, `str`, `None`, and `list[int]`. Integer arithmetic keeps arbitrary precision, floor division, and Python's remainder convention. Booleans are a distinct static type in this subset: using one in an integer slot requires an explicit conversion. Integer object identity and general object introspection are not exposed. Lists have identity, shared mutation, negative indexing, and checked bounds. An alias cannot change the element type of a list.

The initial control flow is annotated functions, assignments, conditionals, loops, and direct calls. The prototype starts with one source module and a fixed set of builtins. Its exact accepted syntax is documented alongside the implementation. Unsupported syntax is rejected, including unsupported syntax in otherwise unreachable branches.

Compilation fixes function identities and signatures. The language does not initially include arbitrary imports, `eval`, `exec`, namespace mutation, monkey patching, reflective attribute access, user-defined classes, metaclasses, decorators, generators, coroutines, or `Any`. These are scope decisions, not invitations to silently approximate Python behavior. Classes and additional protocols can be added with explicit rules for field layout, typed mutation, dispatch, and reentrancy.

We retain ordinary value semantics where supported: arbitrary-precision integers, left-to-right expression evaluation, short-circuit control flow, alias-visible list changes, and bounds errors. The first executable reports an uncaught runtime error and exits; catchable exceptions and tracebacks are later work. A future exception implementation must preserve cleanup on every exit path.

## Compiler

The pipeline is Ruff's cached parser and ty's semantic analysis, a deliberately small Relar contract checker, a typed intermediate representation, ownership lowering, textual LLVM IR, and LLVM optimization and native linking. We pin the Ruff/ty component crates together and use the system LLVM 18 Clang driver to compile the generated IR. Clang does not compile the Python source or generated C.

We embed ty's Salsa database, vendored type definitions, semantic checking, and diagnostic rendering. Relar consumes the same parsed Ruff AST after ty succeeds. The second checker implements only the accepted type language: a general Python type checker's inferred types, stubs, `cast`, or user-defined narrowing functions would not by themselves establish the runtime proofs needed by the backend. Exact runtime types, effects, and storage decisions remain explicit in Relar. Suppression comments cannot waive Relar's runtime type contract.

The IR records resolved calls, expression types, variable slots, control flow, and supported runtime operations. The backend only accepts checked IR. Integer operations get a machine-integer fast path and an already-compiled arbitrary-precision fallback. A fallback continues the operation at that point; it does not restart earlier side effects. LLVM overflow flags and alias metadata are emitted only when their preconditions have been established.

The initial native calling convention uses concrete typed values and borrowed arguments. Returned reference-bearing values are owned. Each expression and local has an explicit lifetime; assignment publishes the new value before releasing the old value. An active call owns its inputs until completion. Optimization of reference counts must preserve those rules.

## Runtime

The runtime provides integers, strings, integer lists, input, output, and checked errors. Heap allocations do not move. Reference counting provides predictable ownership; the first supported types cannot form reference cycles. Introducing recursive containers, closures, or user-defined object graphs requires a cycle-collection design before claiming those graphs are reclaimable.

Small integers stay in registers. Larger integers use a shared owned allocation. List storage owns its elements; reads acquire a reference, and writes acquire the replacement before releasing the previous element. String and list aliases share one allocation. The generated program never holds a pointer into resizable list storage.

Rust helps protect the runtime implementation, but it does not prove emitted LLVM IR safe. Unsafe handle operations stay in a narrow runtime boundary with documented contracts. Differential tests, ownership tests, and sanitizer runs must cover generated code as well as runtime code.

The scriptc failures that motivated this project were failures of lifetime and mutation reasoning across callbacks. Before adding callbacks or finalizers, runtime operations and IR instructions must distinguish operations that can reenter user code. Borrowed container storage cannot survive such an operation; callbacks must remain rooted while running; collection iteration must define how mutation is observed. Dropping a reference can itself become a reentrant operation.

## Room for extensions

The initial implementation does not emulate CPython or load extension modules. Future extension support targets selected extensions rebuilt against our headers; unchanged CPython wheels are not an initial compatibility promise.

Keep the internal native calling convention separate from any future boxed C-facing convention. Keep reference ownership and object identity explicit. Keep allocation, object operations, and errors behind runtime interfaces. These choices allow later checked adapters and callable wrappers without putting C-API calls in every optimized operation today.

We do not commit native containers to CPython's internal layouts. A later adapter must preserve shared mutation and identity, or make conversion explicit. A mutable foreign list cannot become a trusted `list[int]` merely because its current elements passed a one-time check. The eventual boundary must mediate writes, use checked reads, or make an explicit independent copy.

## Prototype and evidence

The first program is a weighted-grid route planner with an ordinary Python implementation of a binary heap and Dijkstra's algorithm. It exercises function calls, loops, arithmetic, shared mutable lists, bounds checks, and data-dependent control flow. CPython executes exactly the same `.py` file and receives identical input.

Correctness gates precede timing: full output comparisons, multiple grid sizes and seeds, an independent small-grid oracle, arithmetic edge cases beyond native integer range, negative indexing, mutation aliases, evaluation order, and rejection of unsupported or ill-typed programs.

Measurements retain raw alternating paired samples, input and source hashes, compiler and interpreter versions, host and CPU information, executable hashes, and compilation time. Report the timing boundary explicitly. End-to-end process timing includes startup and output for both implementations; compilation is measured separately. No performance multiplier is promised in advance.

## Delivery

Use one linear stack of draft GitHub pull requests, following Trellis: each branch targets the preceding branch, and local branch upstreams track that base rather than the published feature branch. Start with this contract, then the compiler/runtime, then the complete example and benchmark evidence. Keep each layer reviewable and record its validation.
