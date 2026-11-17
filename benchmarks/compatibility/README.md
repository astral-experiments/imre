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

## Script shards

Use one record per **original command**, not one per file or diagnostic. Give `id` a globally unique stable command identifier. `source` is the original repository URL, full commit, repository-relative path, and SHA-256 of that source file. `normalized_invocation` records stable cwd, argv and environment with documented placeholders; each run also records its exact argv and evidence. Stock must be exact 3.14.5. Record exit code and SHA-256 of raw stdout/stderr; hash a canonical effects manifest describing files and other observable effects, and link the manifest. Compare each of exit code, stdout, stderr and effects against stock as `exact`, `different` or `not_checked`. If normalization is necessary, retain raw observations and document the comparison method in evidence; do not call a normalized or partial match `exact`. A paired script requires three `pass` outcomes, all four exact comparisons at both levels, no omission, and matching observed values. Commands without receipts remain not run.
