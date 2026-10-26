# Cache, wrapper, and annotation integration runs

Each source attempted all 325 unchanged primary and four free-threading occurrences at O0 and O3. Counts match between levels. CPython ran the fixtures for every native body. None of these runs has a fully native fixture, reference-only pass, reference skip or exclusion. All four free-threading occurrences stop before worker startup.

| Source | Occurrence reports | Native body with host fixture | Host-driven native operation | Mixed | Blocked |
| --- | --- | ---: | ---: | ---: | ---: |
| [#706](https://github.com/astral-sh/imre/pull/706) cached annotations, `8b26d0c` | [325](8b26d0c/evidence-linux.json) + [4](8b26d0c/evidence-free-threading-linux.json) | 6 | 13 | 15 | 291 |
| [#844](https://github.com/astral-sh/imre/pull/844) wrapper links, `4f666a7` | [325](4f666a7/evidence-linux.json) + [4](4f666a7/evidence-free-threading-linux.json) | 4 | 13 | 15 | 293 |
| [#949](https://github.com/astral-sh/imre/pull/949) live len in iterators, `019960d` | [325](019960d/evidence-linux.json) + [4](019960d/evidence-free-threading-linux.json) | 6 | 13 | 13 | 293 |
| [#1027](https://github.com/astral-sh/imre/pull/1027) Python optimization; local merge, `8869c97` | [325](8869c97/evidence-linux.json) + [4](8869c97/evidence-free-threading-linux.json) | 4 | 13 | 15 | 293 |
| [#650](https://github.com/astral-sh/imre/pull/650) deferred annotations; local merge, `7af5331` | [325](7af5331/evidence-linux.json) + [4](7af5331/evidence-free-threading-linux.json) | 4 | 13 | 15 | 293 |

The two additional #706 bodies are the C and Python `test_get_annotations` methods. Their `with_forwardref` counterparts stop at the original `EqualToForwardRef.__eq__` helper. The two #949 bodies are the C and Python `test_lru_reentrancy_with_len` methods; they exercise real cache lookups while `builtins.len` is replaced and restore the original function. Their native bodies replace two mixed observations. The wrapper-link ownership and weak-reference controls in #844 are separate from the suite counts.

The [Format enum method](7af5331/annotation-format-linux.json) and [three ForwardRef error methods](7af5331/annotation-forwardref-linux.json) run under CPython hooks. They are outside both functools denominators. On the same candidate the `Path.stat` comparison and all 135 native path checks (49 rejections) passed at both levels.

The #1027 candidate exercised Python optimization flags independently of Imre's code optimization. Its canonical squash is `e3eec5878e484ed984e4459e92974410d4f5ac73`; the counts here belong to the listed pre-merge source.

[Exact Git trees, parents, and binary hashes](cache-wrapper-and-annotation-sources.json) identify the build used for each run.
