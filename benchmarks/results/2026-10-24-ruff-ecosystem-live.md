> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff ecosystem check on 58 Git repositories

Imre compiles Ruff’s ten Python modules from their pinned upstream sources. The command runs Ruff 0.15.0 and 0.16.1 on every default checkout. With the original Git revisions held fixed, both native and CPython commands reported 33,677 added violations, 4,290 removed violations, 30 changed projects, 22 unchanged projects and six project errors.

| Complete command | CPython 3.12.13 | Imre |
| --- | ---: | ---: |
| Cached, local origins, parallel file walks | 12.20 s | 10.34 s |
| Cached, local origins, single CPU | 131.24 s | 124.21 s |
| Fresh public GitHub clones | 54.08 s | 73.73 s |

The parallel cached row is the median of three pairs. Two pairs matched byte-for-byte. One matched at 33,614 bytes and one at 33,618 bytes. In the third pair Ruff’s parallel directory walk reported different invalid Airflow configurations first. Each engine produced both versions across the three runs. With all processes pinned to a single CPU, two rounds in alternating order produced the same 33,614 bytes in all four runs. The single-CPU row gives the median for each engine.

The two fresh runs include GitHub latency, checkout, and both Ruff processes. Superset and Home Assistant moved between runs. The later run consequently reported four additional violations. Peak RSS including child commands was 688 MiB for each fresh run.

[Reproduce the check](../../examples/ruff_ecosystem/README.md#run-the-original-check-command) · [Wall times, public revisions and stdout hashes](2026-10-24-ruff-ecosystem-live.json.gz) · [CPython, Codon and Rust frozen report comparison](2026-10-24-ruff-ecosystem.md)
