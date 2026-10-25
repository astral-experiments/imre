# Native comparator sort runs

The complete unchanged suite and its free-threading companion ran at O0 and O3 on the two sources below. Every counted native body used CPython for its original module, class, and instance hooks. Direct API controls are outside the denominators.

| Source and suite | Native bodies with host fixtures | Host-driven native operations | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: |
| [#1084](https://github.com/astral-sh/imre/pull/1084), [b260b17](https://github.com/astral-sh/imre/commit/b260b1748f1e885240f78e15f2ac79f2ba09e5a1), [primary 325](b260b17/evidence-linux.json) | 8 | 13 | 15 | 289 |
| [#1084](https://github.com/astral-sh/imre/pull/1084), [b260b17](https://github.com/astral-sh/imre/commit/b260b1748f1e885240f78e15f2ac79f2ba09e5a1), [companion 4](b260b17/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |
| [#1087](https://github.com/astral-sh/imre/pull/1087), [ac7ca6f](https://github.com/astral-sh/imre/commit/ac7ca6f640d4d6b72e4787046e98d58bf0dbdcc5), [primary 325](ac7ca6f/evidence-linux.json) | 10 | 13 | 15 | 287 |
| [#1087](https://github.com/astral-sh/imre/pull/1087), [ac7ca6f](https://github.com/astral-sh/imre/commit/ac7ca6f640d4d6b72e4787046e98d58bf0dbdcc5), [companion 4](ac7ca6f/evidence-free-threading-linux.json) | 0 | 0 | 0 | 4 |

The two `test_sort_int` occurrences use method SHA-256 `ebd328c0d3d8f16909cc29f19c310d4edabff522bf635a4e591b30cf7cdf288c`. The two `test_sort_int_str` occurrences stop at the mixed list literal on the first source and complete on the second; their unchanged method SHA-256 is `918c6372f243ff498eeae9cdb21c80d29aa2ca75ff43628d71ac384b14daa517`.
