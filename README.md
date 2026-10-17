# Relar

Experiments in ahead-of-time compilation of a typed Python subset with LLVM and an independent runtime.

The [initial design](docs/design.md) defines enforced types, deliberately restricted dynamic behavior, explicit ownership, and a path to supporting rebuilt extensions later. The first experiment compiles our own weighted-grid route planner and compares the unchanged Python source against CPython.
