# Comparator and annotation integration runs

Each frozen source ran every unchanged method at O0 and O3. All counts below match between the two levels. Every native method ran under the original CPython fixture hooks.

| Compiler and suite | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [5e3fc39](https://github.com/astral-sh/imre/commit/5e3fc3971562fdacd8971849bf9e401ae6e0784c) [primary, 325](5e3fc39/evidence-linux.json) | 4 | 13 | 15 | 293 |
| [5e3fc39](https://github.com/astral-sh/imre/commit/5e3fc3971562fdacd8971849bf9e401ae6e0784c) [free-threading, 4](5e3fc39/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [6fcf33f](https://github.com/astral-sh/imre/commit/6fcf33f02db9cc7131d73503d52f7bb78fd7b5b9) [primary, 325](6fcf33f/evidence-linux.json) | 4 | 13 | 15 | 293 |
| [6fcf33f](https://github.com/astral-sh/imre/commit/6fcf33f02db9cc7131d73503d52f7bb78fd7b5b9) [free-threading, 4](6fcf33f/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [ff8a252](https://github.com/astral-sh/imre/commit/ff8a2528458cdcf68113bbcbdb8139b8dbec507c) [primary, 325](ff8a252/evidence-linux.json) | 6 | 13 | 15 | 291 |
| [ff8a252](https://github.com/astral-sh/imre/commit/ff8a2528458cdcf68113bbcbdb8139b8dbec507c) [free-threading, 4](ff8a252/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |

On `ff8a252` the unchanged `TestCmpToKeyC.test_obj_field` and `TestCmpToKeyPy.test_obj_field` bodies and assertions run in Imre. The earlier typed-key source `5e3fc39` resolved their fixture but stopped at the local comparator. Their unchanged source hash is `f0fb254775fedbf01391c392e4efecceae0b5f9e549cd43e82bb8b91cc14f22e`.

The [Format method](6fcf33f/annotationlib-format-linux.json) and [three ForwardRef argument methods](6fcf33f/annotationlib-forwardrefs-linux.json) are separate annotationlib controls for the exact Path.stat integration. They are outside both functools denominators.
