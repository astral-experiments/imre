# Native-only program admission

Program checks and builds accept only fixed native lowering. A checked program
that needs registered Python frames, a runtime module launcher, or the general
Python object runtime fails before registered code generation. The diagnostic
keeps the original lowering reason when one exists. A program can fail even if
the operation requiring the runtime lies on a branch that a particular input
never takes. There is no command-line or environment override.

This policy applies to whole, split and ThinLTO layouts, to incremental checking,
and to source and file library entry points. The compiler's executable and
object cache recipes have changed so previous builds cannot satisfy a strict
request. The normal native runtime still supplies operations such as integer
arithmetic, list access, strings, and error handling. The policy governs the
compiler's choice of program representation. It does not audit arbitrary
external native archives.

This is a deliberate compatibility change. Code previously accepted through
the general Python runtime will now be rejected. Successful fixed lowering
does not imply a speed guarantee. The existing provider-generation tooling is
separate from application entry admission.
