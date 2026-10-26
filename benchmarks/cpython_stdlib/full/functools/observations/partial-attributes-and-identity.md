# Native partial metadata and the original join fixtures

Every complete source below used pinned CPython 3.14.5. Both optimization levels have 325 primary and four companion occurrences. The companion remained blocked. No fixture in this table was fully native.

| Source | Native body, host fixture | Host-driven native operation | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [4c74a4e](4c74a4e/evidence-linux.json) — Deferred annotations with the runtime iterator-effects repair | 4 | 13 | 15 | 293 |
| [d0b8740](d0b8740/evidence-linux.json) — Partial fields retain module function identity; differently bound str.join local still rejected | 2 | 13 | 15 | 295 |
| [2e5aaf0](2e5aaf0/evidence-linux.json) — Partial local reassignment restores the two str.join bodies; conditional Placeholder diagnostic remains | 4 | 13 | 15 | 293 |
| [2282db9](2282db9/evidence-linux.json) — Partial attributes, reassignment and all-partial unions, #1248 | 4 | 13 | 15 | 293 |

The two temporary regressions are exactly the accelerated and pure-Python `TestPartial.test_with_bound_and_unbound_methods` occurrences. Both complete bodies, their operations and their assertions ran natively again on the last two sources. Their original CPython fixture hooks still ran on the host. The final source matches the per-occurrence outcomes and roles of canonical control `e1dff5f` at both levels.

The separate `partial-identity-control.json` files compare a supplemental fixture with both genuine reference modules. On `a3f6d0b`, three module-function identity observations differed; `d0b8740` matched all 121 reference bytes but still lost the two unchanged join bodies. The expanded conditional control initially failed to compile on `2e5aaf0`; on `2282db9` it matched all 129 reference bytes. These programs never add to the upstream denominator.
