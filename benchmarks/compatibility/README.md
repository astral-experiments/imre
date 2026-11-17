# Pinned compatibility receipts

The scorecard is an intersection at Imre `49f0dc2ef78b82692e333252faf71b4fc7730949`, on Linux x86-64, against **stock CPython 3.14.5** (`v3.14.5`, commit `5607950ef232dad16d75c0cf53101d9649d89115`). It does not turn cumulative credits, compiler diagnostics, reconstructed fixtures, skips or exclusions into current passes. The 45 historical family denominators and mixed-revision credits come from the frozen TSV named in `cohort.json`; errno adds a separate three-original pilot. No original IDs are invented for unreplayed families. Their whole raw denominators remain not run.

`stdlib/*.jsonl` contains one record per original test ID and occurrence. `scripts/{uv-a,uv-b,ruff-a,ruff-b,ty}.jsonl` is reserved for the separately coordinated 84-command replay; absent files mean not run. `schema.json` documents both record shapes. `scorecard.py` validates them and generates the marked sections in README and `docs/stdlib.md`:

```sh
python3 benchmarks/compatibility/scorecard.py
python3 benchmarks/compatibility/scorecard.py --check
```

## Cloud setup and stdlib replay

Run Imre only in the configured Cloud environment. Fetch the public pin, create a `charlie/` worktree, and build compiler and runtime together from that revision. The cached release executable is not evidence for this pin.

```sh
git fetch --no-tags origin main
git worktree add -b charlie/my-replay /workspace/scratch/my-replay 49f0dc2ef78b82692e333252faf71b4fc7730949
git branch --set-upstream-to=origin/main charlie/my-replay
cd /workspace/scratch/my-replay
export CARGO_HOME=/workspace/.toolchains/cargo RUSTUP_HOME=/workspace/.toolchains/rustup
export PATH=/workspace/.toolchains/cargo/bin:$PATH
cargo build --offline --release --locked -p imre-compiler -p imre-runtime --target-dir /workspace/scratch/imre-build-target
cd /workspace/scratch/imre-scorecard # checkout containing this runner
uv python install 3.14.5 --install-dir /workspace/scratch/python --cache-dir /workspace/scratch/uv-cache --no-bin --system-certs
curl -fL https://codeload.github.com/python/cpython/tar.gz/5607950ef232dad16d75c0cf53101d9649d89115 -o /workspace/scratch/cpython-3.14.5.tar.gz
sha256sum /workspace/scratch/cpython-3.14.5.tar.gz # 0d838be32b52da92a4293ecd99bc91f7be8b4acc573423665bd1a1428be46e47
tar -xzf /workspace/scratch/cpython-3.14.5.tar.gz -C /workspace/scratch
PY=/workspace/scratch/python/cpython-3.14.5-linux-x86_64-gnu/bin/python3.14
CPY=/workspace/scratch/cpython-5607950ef232dad16d75c0cf53101d9649d89115
"$PY" -VV
python3 benchmarks/compatibility/replay_stdlib.py \
  --python "$PY" --cpython "$CPY" \
  --imre /workspace/scratch/imre-build-target/release/imre \
  --runtime /workspace/scratch/imre-build-target/release/libimre_runtime.a \
  --clang /workspace/.toolchains/imre-x86_64-unknown-linux-gnu/imre-toolchain/bin/clang \
  --family errno --manifest benchmarks/cpython_stdlib/full/errno/UPSTREAM.json
```

The runner verifies the upstream Git blob and exact interpreter version, runs the intact FILE once per tier with its full original fixture tree available via `PYTHONPATH`. For Imre it stages the complete, unchanged `Lib/test` tree under a root containing only `test`, so upstream copies of `unittest` and other stdlib modules do not shadow Imre’s shipped stdlib, saves raw stdout/stderr and exact invocations under `evidence/FAMILY`, and writes individual IDs with occurrences. Use `--source Lib/test/...` for package layouts and `--include-module NAME` for explicitly registered fixtures. A build failure records not run, never pass. A suite timeout keeps only completed individual results; unreported originals remain not run. The runner deliberately fails on output it cannot parse or a mismatched unittest total. For multi-file families, coordinate unique evidence/output paths and merge by `(family, id, occurrence)`; do not overwrite a family receipt file or silently shrink its raw denominator.

## Finite unittest launcher on current main

When the runpy entry cannot link its `exec` body, generate a launcher from an explicit ordered list of test modules. For a package, list its leaf modules in stock discovery order and compare the resulting inventory with stock package discovery. The generated file contains ordinary literal imports and uses `TestLoader.loadTestsFromModule`, `TestSuite`, and `TestResult`; it does not implement a module-name resolver.

```sh
python3 benchmarks/compatibility/generate_unittest_runner.py \
  --module test.test_fnmatch \
  --module test.test_tomllib.test_data \
  --module test.test_tomllib.test_error \
  --module test.test_tomllib.test_misc --output /tmp/finite_runner.py
"$imre" build /tmp/finite_runner.py --module-root /tmp --extension-path "$root" \
  --no-cache --opt-level 0 --runtime "$runtime" --clang "$clang" -o /tmp/finite_runner
PYTHONPATH="$root" PYTHONDONTWRITEBYTECODE=1 \
  python3 benchmarks/compatibility/collect_unittest_results.py --timeout 180 \
  --output /tmp/originals.json --stdout /tmp/originals.stdout --stderr /tmp/originals.stderr \
  -- /tmp/finite_runner
```

The capture utility records the collected IDs and occurrence numbers, each completed outcome, fixture errors and subtest skips. If the process times out during an original, that original is marked `timeout`; later originals remain `not_run`. Raw streams, process status, timeout and stream hashes are retained. An exit before collection cannot create original credit. This experiment is at current main, outside the scorecard's fixed Imre cohort; do not add its observations to the pinned scorecard without a matching replay and architectural controls.

## Script shards

Use one record per **original command**, not one per file or diagnostic. Give `id` a globally unique stable command identifier. `source` is the original repository URL, full commit, repository-relative path, and SHA-256 of that source file. `normalized_invocation` records stable cwd, argv and environment with documented placeholders; each run also records its exact argv and evidence. Stock must be exact 3.14.5. Record exact exit, raw stdout/stderr and a raw JSON effects manifest with verified SHA-256 digests. Compare each dimension against stock as `exact`, `different` or `not_checked`. Strict paired credit also requires independently reviewed frozen fixture and reference provenance, as described below; current triple agreement alone is insufficient. Commands without receipts remain not run.

### Migrating a script shard

The source script's SHA-256 does not establish the command's fixture. A reconstructed stock/O0/O3 triple, even if all outputs agree, is evidence about that reconstruction only. Every script receipt must declare `fixture_provenance.status` as `unknown`, `reconstructed`, or `exact_frozen`. Keep unavailable commands as `not_run` (or their actual error/timeout classification), and keep their comparisons `not_checked` where observations are missing. To conservatively migrate an older shard, set `fixture_provenance` to `{"status":"unknown"}` and `paired` to `false`; retain the original outcomes and evidence. Do not promote an entry from its script hash or from agreement among current runs.

For each captured observation, provide exactly one raw representation: `stdout` (UTF-8 text), `stdout_base64`, or `stdout_path`; likewise for `stderr` and `effects_manifest`. Paths must point to small raw files inside this repository. `stdout_sha256`, `stderr_sha256`, and `effects_sha256` must hash the exact bytes of those representations. The effects manifest must decode to a JSON object, not a filename or prose label. Pass outcomes require all three observations and an exit code; any supplied observation is checked even for failures. Keep large artifacts and source checkouts out of Git.

A strict pair additionally requires an independently reviewed entry in `benchmarks/compatibility/frozen/scripts.jsonl`, using `frozen-script.schema.json`. The replay shard cannot supply its own authority. Each anchor binds `id`, `source`, and `normalized_invocation` to an immutable frozen evidence `locator`, plus a SHA-256-verified `raw`, `raw_base64`, or repository-local `raw_path` JSON excerpt. That excerpt contains the same `id`, `source`, and `normalized_invocation`, a nonempty `fixture_scope`, a complete `preimages` map (repository-relative path to SHA-256, or null for an absent input), and a `reference` object with frozen exit code, raw stdout/stderr, raw effect-manifest JSON and their digests. The coordinator must review the excerpt against the frozen campaign evidence before adding an anchor. This separate registry is the trust boundary; the validator checks consistency, not the authenticity of an unreviewed external locator.

For `exact_frozen`, the receipt also supplies `fixture_provenance.preimages.stock`, `.O0`, and `.O3`. All three maps must equal the frozen map, and the current stock exit/output/effects must equal the frozen reference. Only then can exact stock/O0/O3 comparisons produce `paired:true`. For ty-05, the reviewed frozen fixture must include the `dist-workspace.toml` preimage beginning `02d617` and the frozen stock README postimage beginning `8d6a18`; a reconstructed postimage beginning `ea831c` cannot pair. No frozen anchors are inferred or generated from replay outputs.
