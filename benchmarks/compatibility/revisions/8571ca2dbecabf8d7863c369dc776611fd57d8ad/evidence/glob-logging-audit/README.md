# Independent audit of the unactivated glob and logging stage

Audit base: public Imre main `702074e303411dad9392e2b6c4039fd66c44da45`. The compiler snapshot is [Imre `8571ca2dbecabf8d7863c369dc776611fd57d8ad`](https://github.com/astral-experiments/imre/commit/8571ca2dbecabf8d7863c369dc776611fd57d8ad); the original stage was published in [`78882d44be048549417d9f319a7b3de214867828`](https://github.com/astral-experiments/imre/commit/78882d44be048549417d9f319a7b3de214867828). The audit does not activate either family or modify the stage. A staged `paired: true` or the word “verified” in its historical evidence README is a claim, not a validated snapshot pair.

| Family | Frozen originals | Staged IDs | Stage triple-pass candidates | Validated receipts before → after | Strict pairs before → after |
|---|---:|---:|---:|---:|---:|
| glob | 24 | 24 | 21 | 0 → 0 | 0 → 0 |
| logging | 272 | 2 | 1 | 0 → 0 | 0 → 0 |
| Snapshot total | 16,225 | — | — | 3,141 → 3,141 | 1,254 → 1,254 |

The glob stage also has two skips and one native timeout; logging has one stock-only original and lacks staged IDs for the other 270. None earns credit. The distinct release score is outside this compiler snapshot.

## Reproducible identity and source audit

`audit.json` lists every staged ID, occurrence, outcome and exact result-line locator. The audit checks identity/source against the frozen original inventory, without copying historical native outcomes or credits. It checks the original bytes of `stdlib/glob.jsonl` (SHA-256 `61ec05c7664af546ab94a573a3ba97457b34e4a08f1f881761fad06788806a9c`) and `stdlib/logging.jsonl` (`4dded3145629d25c5e7c6a74e0f386c94408d3338dc851dbedd1c6df03c1a210`). The public immutable CPython 3.14.5 source anchors are [test_glob.py](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_glob.py) and [test_logging.py](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_logging.py); the audit verified their SHA-256 and Git blob hashes against the full local source tree.

The glob stage uses the intact upstream FILE launcher. The logging FILE failed to build natively on unrelated diagnostics. Its sole candidate instead uses a reduced `BufferingFormatterTest` leaf. The leaf's selected `setUp`, `test_default`, class header and `setUpModule` match the original source, but the class/file are not intact and the staged stock invocation selected the original FILE rather than the same leaf launcher. Its separate leaf gate cannot be replaced by the FILE gate.

## Why the 22 candidates remain uncredited

The stage retains imported annotation-poison diagnostics (`status: int = True`, “expected int, got bool”) and metadata reporting no image, plus guarded withdrawal reruns. It does **not** retain the required raw supervisor process/open traces tying each saved image to source withdrawal and proving no Python fallback, nor a separately declared direct annotation-poison diagnostic/no-image control. The original four candidate images are absent from their recorded Cloud paths:

| Launcher | Tier | Original image SHA-256 |
|---|---|---|
| glob FILE | O0 | `a912f44f4759e27038a67d67779a81b1a9caebbd01cda4a4112eef87e4db78bd` |
| glob FILE | O3 | `0b6bcea9a8caae3d4fd3bb56798ed5e6369dba62f4213d26fb246809fde172f5` |
| logging leaf | O0 | `20b79eba46717bcbbb635b35437e921a29723693f6261263aac53efc3b6cd681` |
| logging leaf | O3 | `95a0a1b970476ade3b29e15c0973c8a9eefb988c4981911fb5e7c1ffec3903f5` |

The stage also recorded CLI/runtime hashes `36da7044…a9dbe7` / `e3742681…b68764`; the current mounted release artifacts differ. An offline build from a clean checkout of the exact pinned source succeeded after cleaning the compiler/runtime packages in a target seeded from the cached mounted target; `pinned-build.json` and `pinned-build.raw` record its CLI/runtime hashes `97cbb308…641483` / `bf866ff9…c33e9f`. It was built but not executed. New images from that build would require new observations; they could not authenticate the old staged images or runs.

A bounded fresh stock-only replay used exact CPython 3.14.5 and the original source imported by `glob-stock-launcher.py`. All 21 selected glob IDs passed. `glob-stock.json` records the exact invocation, flags, environment, interpreter/source/launcher digests and result-stream digests; `glob-stock.stderr` retains ordered collection/start/result events. No native observation or paired pass is inferred from that replay.

To earn credit, perform a fresh, bounded replay using the same positively bound launcher on exact stock, O0 and O3; retain exact pin/build/source/image identities and raw outcomes; collect both direct and imported wrong-int diagnostic/no-image controls; and capture raw process/open traces for each saved image while the complete test source and bytecode tree is withdrawn, with restoration verified. Activate only independently validated original IDs via a separate verified receipt overlay, preserving the original staged bytes.

From the repository root:

```sh
python3 -B benchmarks/compatibility/audit_staged_glob_logging.py --check \
  --cpython-root /path/to/exact/cpython-5607950ef232dad16d75c0cf53101d9649d89115
python3 -B -m unittest discover -s benchmarks/compatibility -p test_staged_glob_logging.py -v
python3 -B benchmarks/compatibility/scorecard.py --snapshot 8571ca2dbecabf8d7863c369dc776611fd57d8ad --check --root-summary
```

Omit `--cpython-root` to check repository-local stage and stock evidence without the external source tree.
