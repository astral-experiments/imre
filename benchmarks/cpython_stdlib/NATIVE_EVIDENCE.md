# Native stdlib evidence, version 1

The [shared runner](SUITE_RUNNER.md) emits one JSON report per optimization level.
The transport is an array of two reports. Each report has schema
`imre.cpython-stdlib.native-evidence/v1`, the suite name, the full CPython commit,
the reference platform, `opt_level` (0 or 3), the occurrence denominator, the
distinct ID count, classification counts and a `records` array. The pinned
[unittest output](full/unittest/native-linux.json) is a complete example.

## Consuming a report

Use the [validator](suite_evidence.py) before aggregating. It checks the commit,
platform, both optimization levels, counts, all occurrence keys, every original
reference outcome and the classification derived from the four execution roles.

```python
import json
from pathlib import Path
from suite_evidence import validate_pair

reference = json.loads(Path("full/unittest/reference-linux.json").read_text())
reports = json.loads(Path("full/unittest/native-linux.json").read_text())
validate_pair(reports, reference)
for report in reports:
    for row in report["records"]:
        key = (report["suite"], row["id"], row["occurrence"], row["opt_level"])
        # Include report["platform"] and report["cpython_commit"] when combining targets/releases.
        if row["classification"] == "fully-native":
            pass  # only this category counts toward fully native compatibility
```

From the repository root, the same validation is available without dependencies:

```sh
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/unittest/reference-linux.json \
    benchmarks/cpython_stdlib/full/unittest/native-linux.json
```

## Per-occurrence record

| Field | Meaning |
| --- | --- |
| `id`, `occurrence`, `opt_level` | Original upstream ID, 1-based occurrence in loader order **within that ID**, and 0 or 3. Never deduplicate inherited methods or use source hashes as IDs. |
| `fixture_variant` | Original case class or a more precise variant supplied by the owner. A repeated ID may have different fixture state. |
| `reference` | Exact outcome for this occurrence in the pinned reference: `"pass"` or a one-key object with `skipped`, `failure` or `error` and its original text. |
| `roles` | Four required locations: `body`, `assertions`, product `operation`, and `fixture`. Each is `imre`, `cpython`, `mixed`, `none` (no such component), `unobserved` (not reached) or `unknown` (an interrupted attempt supplied no complete observation). |
| `outcome` | `pass`, `failure`, `error`, `skip`, `blocked`, `excluded`, `not-run`, `expected-failure` or `unexpected-success`. An upstream skip may have native outcome `not-run`. |
| `classification` | Category derived from the reference, observed outcome, roles and any reviewed exception. |
| `evidence` | Passing adapters name the actual tested operation and describe the fixture. Include native call counts, pinned method SHA-256, assertion path, and observed subtest or fixture events where applicable. |
| `reason`, `exception` | Exact diagnostic for failures, blockers or skips. An individually justified exception additionally records its kind, reason, condition, source, platform, configuration, and fixture variant. |

Passing an upstream test involves four separate questions: where its original
body ran, where its checks ran, where the library operation ran, and who supplied
its fixtures. A host that merely starts a binary and checks its exit status need
not supply a fixture or assertion. A host that prepares original case state,
verifies an assertion message or calls a CPython implementation does.

| Classification | Meaning |
| --- | --- |
| `fully-native` | Body and product operation ran in Imre; assertions and fixtures ran in Imre or were absent. |
| `native-body-host-fixture` | Body, assertions and product operation ran natively; CPython supplied some or all fixture execution. |
| `host-driven-native-operation` | Product operation ran in Imre; CPython ran the body or checked the assertions. |
| `mixed` | Body, assertions or the product operation had both native and host execution. |
| `host-operation`, `no-product-operation` | The product operation ran entirely in CPython, or none was observed. Neither earns native credit. |
| `reference-only`, `reference-skip`, `reference-failure` | No native pass. A reported upstream skip keeps its precise original fixture, condition and reason. |
| `blocked`, `failed`, `unexpected-skip`, `reference-disagreement` | A compiler/fixture blocker, an unsuccessful attempt, a skip without matching upstream evidence, or a native pass against a nonpassing reference. |
| `reviewed-exclusion` | This exact occurrence, configuration and optimization level have an individual review and a regression or issue to revisit. Keep the occurrence in the denominator. |

## Producing records

Use `run_evidence(suite, executions, reference, level, ...)` in
[suite_native.py](suite_native.py). An ID selects all occurrences; a tuple
`(id, occurrence)` selects only one. Supply `fixture_variant(case)` when class
identity alone does not identify the fixture. The original `run_in_fixtures`
interface still returns successful callback values, failures and skips.

For a compiled method, pass `operation=("imre", "unittest.TestResult.stop")`
to `native_method`. For a host-driven or mixed adapter, return a dictionary
with explicit `roles` and `evidence` only after exercising the entire original
method. The runner collects the actual unittest result, including setup,
teardown, skips and subtest failures. It refuses successful callbacks that
only supply a legacy summary label. Custom suite execution needs a dedicated
fixture adapter.

For a direct TestCase with default fixtures, supply a verified
[`NativeDefaultMethod`](SUITE_RUNNER.md#running-a-method-with-native-default-fixtures)
under `native_defaults={(id, occurrence): entry}`. Its lifecycle runs inside
the native `unittest.main()` process. A host that discovers the case does not
run the fixture; a host that executes the operation still gets the
`host-operation` classification.

For a compiler rejection, pass `preflight={(id, occurrence): compile_blocker(exc)}`.
For an independently executed suite, use `build_report(reference, level,
discovered, observations, exceptions=...)` directly. Supply the full
`(id, fixture_variant)` discovery sequence and concrete observations keyed by
`(id, occurrence)`; never fill in default native roles for missing callbacks.

Supply exceptions under exact `(id, occurrence, opt_level)` keys. An
`upstream-skip` must match the reference's original reason. A
`reviewed-exclusion` also needs its review and a regression or issue to revisit;
its preflight outcome is `excluded`. Until reviewed, use `blocked` with the
specific diagnostic. A differently worded runtime skip stays an unexpected
skip even when the same ID was skipped by the reference. See
[`reference_skips` in the unittest gate](full/unittest/run.py) for both pinned
signal conditions.
