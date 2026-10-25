# Placeholder, live builtin length and annotation integration

Each frozen source ran every unchanged method at O0 and O3. All counts below match between the two levels. Every native method ran under the original CPython fixture hooks.

| Compiler and suite | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [6fd9333](https://github.com/astral-sh/imre/commit/6fd9333646e3c8136badd4169072850e7f21df66) [primary, 325](6fd9333/evidence-linux.json) | 6 | 13 | 13 | 293 |
| [6fd9333](https://github.com/astral-sh/imre/commit/6fd9333646e3c8136badd4169072850e7f21df66) [free-threading, 4](6fd9333/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [88944a5](https://github.com/astral-sh/imre/commit/88944a5283648e183ef4a06320f8459dbfdf867b) [primary, 325](88944a5/evidence-linux.json) | 4 | 13 | 15 | 293 |
| [88944a5](https://github.com/astral-sh/imre/commit/88944a5283648e183ef4a06320f8459dbfdf867b) [free-threading, 4](88944a5/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| a007825 (local merge) [primary, 325](a007825/evidence-linux.json) | 4 | 13 | 15 | 293 |
| a007825 (local merge) [free-threading, 4](a007825/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| 54d7a26 (local merge) [primary, 325](54d7a26/evidence-linux.json) | 4 | 13 | 15 | 293 |
| 54d7a26 (local merge) [free-threading, 4](54d7a26/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |

On [#949](https://github.com/astral-sh/imre/pull/949), both original LRU methods exercise the actual builtin length monkeypatch, cache lookups and restoration. Their original hooks remain in CPython. On [#1111](https://github.com/astral-sh/imre/pull/1111), all four singleton construction methods stop at line 285 on `self.assertIs(tp(), PH)` even though separate native Placeholder controls match CPython.

Both [#650](https://github.com/astral-sh/imre/pull/650) candidates have feature parent `6fcf33f02db9cc7131d73503d52f7bb78fd7b5b9`. The exact trees are reproducible from these two main parents:

| Main parent | Tested tree | Original annotationlib methods |
| --- | --- | --- |
| [2d37e83](https://github.com/astral-sh/imre/commit/2d37e83ce79d7f35a2942042ffb05b06f44d53b7) | `166129c291fe6b42674116688bf0dfa8db522ccf` | [Format](a007825/annotationlib-format-linux.json), [ForwardRef](a007825/annotationlib-forwardrefs-linux.json) |
| [21fa57f](https://github.com/astral-sh/imre/commit/21fa57fc70eae8d6aebb4add9c8ac6232d86494f) | `11fc49fd9f1cddf5da9054ab087d302d8f074056` | [Format](54d7a26/annotationlib-format-linux.json), [ForwardRef](54d7a26/annotationlib-forwardrefs-linux.json) |

Both candidates matched all eight integration programs at both levels and passed 135 path checks including 49 rejections under ASan. These controls and the annotationlib methods are outside both functools denominators.
