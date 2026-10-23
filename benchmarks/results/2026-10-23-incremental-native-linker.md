# Incremental native linking on the Ruff conformance reporter

The `a4fca479` snapshot is the seven-module reporter on which the original `render.py` edit took 1.546 s. `78259797` is the earlier captured reporter. Each edit changes one equal-length heading in `render.py` and in the independent upstream Python oracle. The report covers 146 test files, 1,169 old diagnostics and 1,201 new diagnostics.

| Reporter | Operation | GNU BFD | Bundled LLD | Paired change in wall time (95% interval) |
| --- | --- | ---: | ---: | ---: |
| `a4fca479` | Cold build | 2.243 s | 1.829 s | -16.9% [-31.6%, -15.7%] |
| `a4fca479` | Unchanged build | 29.02 ms | 31.82 ms | +9.3% [+2.4%, +19.7%] |
| `a4fca479` | Unchanged run | 63.24 ms | 64.72 ms | +1.9% [-1.6%, +7.4%] |
| `a4fca479` | Edited build | 1.272 s | 0.893 s | -28.0% [-35.0%, -26.9%] |
| `a4fca479` | Edited run | 1.276 s | 0.936 s | -26.4% [-28.6%, -25.5%] |
| `a4fca479` | Direct execution | 37.17 ms | 36.28 ms | -1.0% [-3.9%, +0.8%] |
| `78259797` | Cold build | 2.323 s | 1.835 s | -20.9% [-36.0%, -15.7%] |
| `78259797` | Unchanged build | 28.98 ms | 30.40 ms | +7.4% [+1.3%, +11.2%] |
| `78259797` | Unchanged run | 61.63 ms | 63.58 ms | +3.3% [+1.3%, +4.9%] |
| `78259797` | Edited build | 1.203 s | 0.864 s | -27.6% [-28.8%, -26.3%] |
| `78259797` | Edited run | 1.215 s | 0.899 s | -25.9% [-26.6%, -20.5%] |
| `78259797` | Direct execution | 35.61 ms | 35.96 ms | +0.4% [-1.0%, +1.2%] |

Each column shows its median. The percent change and bootstrap interval use paired timings with balanced execution order. Each snapshot ran four cold pairs, 12 pairs per build/run scenario, and 72 direct-runtime pairs on CPU 12 of an AMD EPYC Milan host. Both compilers had the same release runtime archive with line tables and the same Ubuntu LLD 18.1.3 binary beside them. The unmodified control ran GNU BFD 2.42. Process observation verified the selected linkers, seven objects, `-O3`, section GC and full RELRO. All 528 native outputs matched the 26 upstream CPython oracles byte for byte. All 96 edited invocations reused six objects and compiled only `conformance_report.render`.

## Where the edit time goes

On the compiler that produced the historical 1.546 s result, 15 instrumented edits spent 519 ms in the final native link, 486 ms compiling `render.py` and 186 ms generating the seven LLVM units, including 142 ms in ty. Verifying and restoring the six objects took 1.8 ms. The edited `.o` changed only in `.rodata` on all 15 edits. Each instrumented build produced a byte-identical ELF to its paired uninstrumented build.

Four separate process-profile pairs on the final compilers measured the same reporter's Clang link at 575 ms with BFD and 180 ms with LLD. The changed module took 493 and 476 ms to compile, the frontend and IR took 183 and 188 ms, and 43 and 43 ms elapsed between the link driver exiting and Relar exiting. A separate replay on the smaller workspace `dist` runtime included LLD's private input trace and is in the sample file.

## Reproduce

Build the control compiler at `9b8ddc22` and the candidate from this change. Place a byte-identical `librelar_runtime.a` and a copy of the distribution's `ld.lld-18` named `relar-ld.lld` beside each preserved compiler. Use an unmodified checkout of `a4fca479` for the reporter and its official captured inputs. Replace CPU 12 with an available core on the test host.

```sh
python3 benchmarks/compiler_incremental.py --cached-parent \
  --parent /tmp/before/relar --child /tmp/after/relar \
  --parent-source /tmp/reporter --child-source /tmp/reporter \
  --parent-revision 9b8ddc22 --child-revision candidate \
  --cold-pairs 4 --edit-pairs 12 --runtime-pairs 72 --cpu 12 \
  --scratch-parent /tmp --output /tmp/relar-samples.json --summary /tmp/relar-summary.json
```

For the earlier reporter, use `78259797` and pass its prepared `1x/changed` directory with `--inputs`. The per-invocation timings, fingerprints, cache decisions, ELF measurements, and process profile are in the [compressed JSON samples](2026-10-23-incremental-native-linker.json.gz).
