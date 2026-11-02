The upstream test file is the one at CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`, blob `2c99b221e52d1f1c1fb9a18026231127aa683d73`. Each row ends before the first original case callback. The ordinary entry is `imre -B -m discover`. Boxed builds use `--runtime-imports`.

| Diagnostic source tree | Ordinary | Boxed |
| --- | --- | --- |
| Public `06c9fd7d` with only the change from `bb7f44a2` | O0/O3, line 43, missing Concatenate | O0/O3, same |
| `b967fd4`, also with the port of #3872 | O0, line 44, missing TypeGuard | O0, same |
| `b45dbbc`, also with TypeGuard and TypeIs | O0, line 56 imports ann_module695, which stops on line 5 at variadic class type parameters | O0, same |

The temporary latter two trees are on [the public probe branch](https://github.com/astral-experiments/imre/commits/charlie/typing-intact-import-probes). Their reveal_type export alone has incomplete generic metadata. The separate named fixtures and historical 107 are excluded from these observations.

On `8c8ec31c`, the ordinary generic function implementation from #4085, the mixed ParamSpec/TypeVarTuple implementation on #3872, the predicate exports, and the variadic class implementation allow the *unchanged* intact boxed O0 import to complete `ann_module695`. It then stops at `test_typing.py:56`, inside `mod_generics_cache.py:24`, with `NotImplementedError: explicit TypeAliasType construction`. The process exited 1 before any case began. The independent dependency import driver also records all nine fixture outcomes separately. [Diagnostic source](https://github.com/astral-experiments/imre/commit/8c8ec31c4b2537ba446dfb6394a1d7dd2c4ebd15)

`4c4f4a6f` adds the shared special-form subclass hook and the focused non-generic eager constructor. Intact boxed O0 built successfully, then exited 1 at `test_typing.py:56` importing `ann_module.py:43`. Constructing `S` with its custom metaclass raises `NotImplementedError: custom metaclass construction is unsupported`. No original case started. The separate stock control observes the original `Meta` identity and annotations. [Diagnostic source](https://github.com/astral-experiments/imre/commit/4c4f4a6f3a7aaaee615f1c6de5e6932a68ad3a8f)
