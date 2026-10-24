> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Compact diff snapshots in Ruff’s ecosystem checker

On the 10× Sphinx and Prefect replay, peak RSS fell from **514.8 to 249.6 MiB** (51.5% lower). Median time was **2.90 → 2.67 s** in six paired runs against #326. All five frozen Markdown reports matched CPython byte-for-byte. [Parent measurements](2026-10-24-ruff-ecosystem-storage.md) · [Raw samples, hashes and input revisions](2026-10-24-ruff-ecosystem-packed.json.gz)

## Frozen reports

Each cell gives **wall time in milliseconds / peak RSS in MiB**. The last column compares wall time.

| Input | CPython | Codon | Rust | Imre | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: |
| Bokeh preview | 171,401 / 182.9 | 1,135 / 142.9 | 534 / 63.5 | 513 / 112.4 | 0.96× |
| Superset preview | 1,479,775 / 410.7 | 3,808 / 646.1 | 1,430 / 151.7 | 1,364 / 267.1 | 0.95× |
| Zulip preview | 152,859 / 182.2 | 1,317 / 236.4 | 501 / 58.8 | 489 / 104.4 | 0.98× |
| Sphinx + Prefect, 1× | 2,401 / 153.5 | 1,493 / 169.9 | 281 / 52.1 | 403 / 93.3 | 1.43× |
| Sphinx + Prefect, 10× | 22,065 / 534.6 | 13,379 / 372.8 | 2,392 / 60.5 | 2,692 / 249.6 | 1.13× |

The standard workload keeps 20 comparisons alive at 10×. On Linux CPU 3, CPython 3.12.13, Codon 0.20.4, Rust and Imre each ran one warmup and four measured rounds at each size. Preview used six interleaved native rounds after one warmup. The CPython 3.14.5 preview numbers come from the previous exact full-project runs. A separate six-pair parent comparison used the same five inputs on CPU 3. [Capture and replay commands](../../examples/ruff_ecosystem/README.md)

## Reading the complete diff

After a caller selects changed lines from large sorted inputs with few differences, Imre stores how many bytes each row shares with the previous row, followed by its remaining bytes. A tag records which input contains the row. The runtime releases its references to the input strings. Reading the complete diff later restores both ordered lists, computes the intraline guides, and releases the packed data. Reading every row through Ruff’s unchanged `Diff.from_pair(...).lines` took 136 → 151 ms for Sphinx (11% longer) and 347 → 394 ms for Prefect (14% longer). Prefect’s peak RSS went from 82.8 to 89.5 MiB. Reading Prefect’s raw `ndiff` first, before selecting changes, took 394 → 393 ms. The full outputs matched CPython 3.12 and 3.14, including the guides. Tests also read the diff after dropping both input lists and checked that restored duplicates shared an allocation.

## Full 58-project command

The unchanged `ruff-ecosystem --force-preview check` command compared Ruff 0.15.0 with 0.16.1 on 58 pinned checkouts. With three interleaved runs per build, median wall time was **24.43 → 28.39 s** and peak RSS was **684.6 → 683.9 MiB**. Times overlapped across runs and include Git and both Ruff children for each project. All three packed runs reproduced the original 76,952-byte CPython 3.14 report. Both Ruff versions failed on Airflow. One parent run surfaced the other failure and matched its 76,980-byte Python reference. All checkouts remained clean. [Live harness](../ruff_ecosystem_live.py)
