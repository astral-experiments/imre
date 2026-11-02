The upstream test file is the one at CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`, blob `2c99b221e52d1f1c1fb9a18026231127aa683d73`. Each row ends before the first original case callback. The ordinary entry is `imre -B -m discover`. Boxed builds use `--runtime-imports`.

| Diagnostic source tree | Ordinary | Boxed |
| --- | --- | --- |
| Public `06c9fd7d` with only the change from `bb7f44a2` | O0/O3, line 43, missing Concatenate | O0/O3, same |
| `b967fd4`, also with the port of #3872 | O0, line 44, missing TypeGuard | O0, same |
| `b45dbbc`, also with TypeGuard and TypeIs | O0, line 56 imports ann_module695, which stops on line 5 at variadic class type parameters | O0, same |

The temporary latter two trees are on [the public probe branch](https://github.com/astral-experiments/imre/commits/charlie/typing-intact-import-probes). Their reveal_type export alone has incomplete generic metadata. The separate named fixtures and historical 107 are excluded from these observations.
