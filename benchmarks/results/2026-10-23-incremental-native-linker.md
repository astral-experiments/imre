# Incremental native linking on the Ruff conformance reporter

A one-line edit to a Markdown heading makes Relar regenerate LLVM for seven modules, compile the changed `render` module, and link a fresh executable with the Rust runtime. On the compiler that produced the original 1.546 s result, fifteen instrumented edits spent 186 ms in whole-program processing, including 142 ms in ty, 486 ms in Clang compiling `render`, and 519 ms in the final native link. Validating and restoring the six cached objects took 1.8 ms. Only the edited object's `.rodata` varied across those edits. Every instrumented build produced a byte-identical executable to its paired uninstrumented build.

## Reporter timings

Both compilers include upstream commit `9a7f9951`, which changed the compiler, runtime, and reporter. The candidate includes the linker change. We repeated the edit on the same `a4fca479` reporter that produced the 1.546 s result, the earlier `78259797` snapshot, and the updated `9a7f9951` reporter. All three use the same 146 files, 1,169 old diagnostics and 1,201 new diagnostics.

| Reporter revision | Operation | GNU BFD | Bundled LLD | Paired wall-time change (95% interval) |
| --- | --- | ---: | ---: | ---: |
| `a4fca479` | Cold build | 2.182 s | 1.878 s | -14.2% [-16.5%, +0.9%] |
| `a4fca479` | Unchanged build | 31.48 ms | 32.48 ms | +9.1% [+1.1%, +17.1%] |
| `a4fca479` | Unchanged run | 67.12 ms | 67.07 ms | +1.8% [-3.4%, +4.1%] |
| `a4fca479` | Edited build | 1.251 s | 0.907 s | -26.9% [-29.4%, -23.2%] |
| `a4fca479` | Edited run | 1.282 s | 0.944 s | -26.6% [-27.2%, -25.8%] |
| `a4fca479` | Direct execution | 36.28 ms | 36.34 ms | +0.1% [-1.7%, +1.3%] |
| `78259797` | Cold build | 2.293 s | 1.874 s | -17.4% [-24.7%, -3.8%] |
| `78259797` | Unchanged build | 30.82 ms | 31.93 ms | +4.4% [-8.3%, +7.0%] |
| `78259797` | Unchanged run | 64.03 ms | 66.30 ms | +4.3% [+1.2%, +8.2%] |
| `78259797` | Edited build | 1.257 s | 0.900 s | -29.0% [-33.4%, -26.5%] |
| `78259797` | Edited run | 1.252 s | 0.939 s | -26.2% [-27.9%, -13.4%] |
| `78259797` | Direct execution | 36.22 ms | 36.38 ms | +0.5% [-0.8%, +1.1%] |
| `9a7f9951` | Cold build | 2.272 s | 1.920 s | -15.8% [-20.6%, -10.2%] |
| `9a7f9951` | Unchanged build | 29.36 ms | 32.42 ms | +9.2% [+4.6%, +12.2%] |
| `9a7f9951` | Unchanged run | 62.96 ms | 64.76 ms | +1.7% [+0.7%, +3.2%] |
| `9a7f9951` | Edited build | 1.289 s | 0.945 s | -26.3% [-27.9%, -25.3%] |
| `9a7f9951` | Edited run | 1.311 s | 0.969 s | -25.5% [-26.7%, -24.8%] |
| `9a7f9951` | Direct execution | 35.60 ms | 35.55 ms | -0.0% [-0.7%, +0.7%] |

Each snapshot ran four cold pairs, twelve pairs per edit or unchanged CLI scenario, and 72 direct-execution pairs on CPU 12 of an AMD EPYC Milan host. Each column shows its median. The percentage change and bootstrap interval use within-pair timings and balanced execution order. Both compilers used the same physical 84,715,672-byte release runtime archive and had the same Ubuntu LLD 18.1.3 companion beside them. The control linked with GNU BFD 2.42. The selected linkers, seven object inputs, `-O3`, section GC and full RELRO were checked on the real commands. All 792 native reports matched their 39 upstream CPython references byte for byte. Every one of the 144 edited invocations compiled only `conformance_report.render` and reused six objects. [Per-invocation wall and CPU time, cache decisions, ELF sizes and fingerprints](2026-10-23-incremental-native-linker-9a7f9951.json.gz)

## Compiler and linker profile

Four separate process-profile pairs on the original `a4fca479` workload measured the complete Clang link at 523 ms with BFD and 168 ms with LLD. Compiling the changed module took 473 ms and 469 ms. The frontend and IR took 183 ms and 181 ms, and 40 ms and 41 ms elapsed between the link driver's exit and Relar's exit.

The actual edited LLVM module was 335,280 bytes and contained 48 function definitions. A time trace on CPU 18 spent 224 ms optimizing LLVM IR and 217 ms generating target code. All eighteen timed replays produced the exact original object. With the smaller distribution-profile runtime archive and the updated reporter, a separate fifteen-pair native link replay fell from 412 ms to 141 ms. The bundled linker and notices added 1.73 MiB to the compressed x86-64 Linux package.

Before upstream `9a7f9951` landed, the same `a4fca479` workload on `9b8ddc22` showed an edited build falling from 1.272 s to 0.893 s (28.0% median paired reduction) and a Clang link falling from 575 ms to 180 ms. Both earlier reporter panels passed all 528 complete output comparisons. [Earlier samples and process profile](2026-10-23-incremental-native-linker.json.gz)

## Reproduce

Build the control compiler at `9a7f9951` and the candidate from this change. Place byte-identical copies of `librelar_runtime.a` and the distro's `ld.lld-18` named `relar-ld.lld` beside both preserved compilers. Check out the same reporter revision in both arms and use its prepared `1x/changed` fixture. Replace CPU 12 with an available core on the test host.

```sh
python3 benchmarks/compiler_incremental.py --cached-parent \
  --parent /tmp/before/relar --child /tmp/after/relar \
  --parent-source /tmp/reporter --child-source /tmp/reporter \
  --parent-revision 9a7f9951 --child-revision candidate \
  --inputs /tmp/fixtures/1x/changed --cpu 12 \
  --cold-pairs 4 --edit-pairs 12 --runtime-pairs 72 \
  --scratch-parent /tmp --output /tmp/relar-samples.json --summary /tmp/relar-summary.json
```
