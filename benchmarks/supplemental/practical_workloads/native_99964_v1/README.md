# Prospective native replay at 99964: Rich withdrawal blocker

This supplemental measurement uses the independently reviewed, immutable stock corpus at Imre commit `99964c6734dbe008d69d5016eb60d25abd31ab0d`. The stock source, fixtures, expectations, and references are unchanged. This is separate from historical84, its three credited pairs, and every pinned compatibility scorecard. **No credit is awarded.** The uv-rich workload fails the required source-withdrawal control, so no credit-granting PR or activation is proposed.

| Frozen command | O0 | O3 | Normalized mutations / HTTP / cargo journal | Awarded |
| --- | --- | --- | --- | ---: |
| ruff-publisher | exact | exact | exact / exact / exact | 0 |
| uv-publisher | exact | exact | exact / exact / exact | 0 |
| uv-rich | exit 1; missing `rich` | exit 1; missing `rich` | exact / exact / exact (no effects) | 0 |
| ty-readme | exact | exact | exact / exact / exact | 0 |

The 6 exact tier observations match independent raw status, stdout, stderr, and all normalized effects. Stock uv-rich exits 0 with `Hello Ruff, from uv!\n`; each source-withdrawn saved native image exits 1, writes no stdout, and reports `ModuleNotFoundError: No module named 'rich'` on stderr. Its 49-file tag-verified frozen source ZIP was absent during execution and restored by hash. A separately labeled archive-present baseline runs the same O0 image successfully and opens the source archive 50 times. A bounded O0 variant with the existing `--include-module rich` option still fails when the archive is withdrawn. Neither auxiliary run earns credit; no uv package or transitive dependency was fetched.

The CLI and runtime were built together offline from a clean checkout at the exact stock commit, using a separate copy of the warm release cache and `/workspace/.toolchains`:
- CLI SHA256: `0d67c2cc7974cb3838224f2dea5963ea88b127f395bf8641030efba9b4079072`
- runtime SHA256: `83d23593bf7b800435fee1b24a6114bb6dace93688d41205b19719ef3a9938dd`

The build manifest and log bind source HEAD/tree, commands, hashes, and toolchain. Each workload has a fresh O0 gate importing a separate first-party `status: int = True` module; all four gates rejected with the expected int/bool diagnostic and no output image. Each saved O0/O3 image was built with the pinned matching runtime and `--no-cache`, receives the frozen program argv and sanitized environment, and executes under explicit umask `0077`. Before-manifests equal the frozen preimages. The C supervisor's traced source/ZIP rename, absent-source checks, image exec, wait status, restoration, and subsequent after-manifests are retained. No workload source or bytecode is delegated to Python. Rich's failed traceback attempts to open its missing entry source; that failed provenance control is retained in its finding rather than hidden.

Publisher runs deliberately execute the pinned CPython cargo fixture twice. They are **not child-free or CPython-free**. The sanctioned interpreter SHA256 is `c27397da5541f67655be462e128ca3d3eb6e0a8eb0d39036896c9042529f4304`; the frozen stub SHA256 is `4a57e30e3cfc44ad1836a66760a844ceb628c68bf52031c2fac6f89e9356e85e`. The validator binds each child to the native workload parent, exact interpreter and stub path, restricted metadata/publish argv, script open, wait, effective-environment journal, and the independent HTTP/cargo/mutation references. No other Python exec is allowed, and native images have no linked or loaded libpython. Only a fake loopback registry is used; no real Cargo publish or public PyPI operation occurred.

Run the offline validator without executing a workload:

```sh
python3 -B benchmarks/supplemental/practical_workloads/native_99964_v1/validate_native.py
```

It first validates stock_v1 offline and checks that its tracked subtree matches the immutable commit. It then recomputes the eight receipt comparisons from raw streams, before/after manifests, request and process journals, gates, and compressed supervisor traces. `evidence/outcome.json` is the uncredited outcome ledger; `evidence/<case>/<tier>/receipt.json` binds exact argv, environment, build/image hashes, evidence, separate comparison dimensions, and provenance findings. `replay_cloud.py.txt` records the Cloud capture source as used; it intentionally requires a clean checkout at the pin and writes outside the corpus.
