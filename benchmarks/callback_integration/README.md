# Two-target callback integration study

`trapezoidal_area` from The Algorithms’ Python library calls `fnc(x_start)` before entering its loop, then `fnc(x2)` at every integration step. The independent workload uses the two integrands and three numerical grids in its published doctests. The integration function, including its arithmetic and control flow, is byte-for-byte upstream source. The harness omits the standalone `__main__` demonstration, selects either callback from an argument, and repeats the original computation. [Pinned upstream caller and inputs](https://github.com/TheAlgorithms/Python/blob/6e03a48579ca212a08b36be00378f887dc41a003/maths/numerical_analysis/numerical_integration.py#L10) · [Complete upstream file](numerical_integration.py) · [MIT license](LICENSE.md) · [Driver](driver.py)

The original doctests return integer `5` and compute `9*x**2`. The driver uses `5.0` for the typed callback’s float ABI and `9*(x*x)` because this Relar compiler rejects float `**`. The latter changes five individual callback results on `[-4, 0]` and six on `[-4, 4]` through rounding. All three final integrals and all repeated checksums match CPython’s original calculations bit-for-bit. The driver places the same two callback definitions either alongside the integrator, as in the upstream module’s demonstration, or in a separate calling module. Both layouts build exactly two cached native objects.

## Recorded default native builds

Main was `eaf1f9a9e12120c065b2e0c289eab986203b6905` (CLI `bf17d573…`). The guarded two-direct-arm CLI was `e3f37263…`. Both used the same unchanged release runtime (`4636abd6…`) and packaged LLVM 18.1.3. The three grids were repeated to produce 2.5 million integration steps per timed process. Each baseline/candidate pair ran on logical CPU 3 of an AMD EPYC-Milan host. The 24 paired blocks per case balanced both execution orders, with one warmup per build and separate paired zero-round processes. Speedups are the median within-block baseline/candidate elapsed ratio. The 95% percentile intervals resample complete complementary two-block groups 10,000 times.

| Published case | Baseline median | Candidate median | Paired speedup (95% interval) | Candidate faster |
| --- | ---: | ---: | ---: | ---: |
| Constant, `[12, 14]`, 1,000 steps | 53.43 ms | 51.01 ms | 1.0465× (1.0389–1.0552) | 22/24 |
| Quadratic, `[-4, 0]`, 10,000 steps | 65.38 ms | 66.17 ms | 0.9877× (0.9836–0.9917) | 4/24 |
| Quadratic, `[-4, 4]`, 10,000 steps | 65.27 ms | 66.17 ms | 0.9894× (0.9874–0.9925) | 5/24 |

With callbacks in the integrator’s module, `fn_m1_0` grew from 1,273 to 1,810 machine-code bytes (+42.2%). The executable `.text` section grew from 471,734 to 472,278 bytes (+0.12%). Captured default LLVM reproduced each original cached object byte-for-byte before tracing. In the ordinary numeric range, the candidate checked the callback target once per integration and executed exactly 1,000 or 10,000 direct calls. The initial callback stayed indirect. With `steps=0` the guard ran once and the loop body never ran. Negative step counts and boxed counts of magnitude `10**400` took the original checked-range path. A huge positive count raised the same `OverflowError` on its conversion to float before the first body callback.

With the callbacks in the other module, the baseline and candidate produced identical objects and complete ELF bytes (SHA-256 `09bc2c9b48de020d62c9853f444d8e78fc06b66d6e92591a672f2e76b100fae9`). Every callback at the cross-module site stayed indirect. The comparison also measured two alternatives in the same-module layout. A target check inside every step gave 0.880×, 0.868× and 0.865× in table order. A loop-entry guard that invoked only the constant arm directly gave 1.030×, 0.943× and 0.939×. [Complete hashes, dynamic counts, timing pairs and intervals](measurements.json.gz)

## Reproduce

Use Linux x86-64, Python 3.12 or newer, `nm`, and `objcopy`. Build the baseline and candidate release CLIs from their respective checkouts using `cargo build --locked --release -p relar-compiler --bin relar`, using separate target directories. Build one release runtime using `cargo build --locked --release -p relar-runtime --lib`. Place a copy of the same Linux release `relar-toolchain` directory next to each CLI, as in a Relar release installation. Supply one `librelar_runtime.a` to both builds. The native benchmark deliberately omits `--clang`, `--emit-llvm`, and `--whole-program`. Each native compile verifies the presence of exactly two cached module objects.

From this directory, first check the retained record, then run the local pair. Supply a fresh work directory for each run. `--blocks 0` checks both layouts, all output bits, code sizes, and the cross-module binary identity without timing.

```sh
python3 replay.py --audit-record measurements.json.gz
taskset -c 3 python3 replay.py \
  --baseline /path/to/baseline/relar --candidate /path/to/candidate/relar \
  --runtime /path/to/librelar_runtime.a \
  --work ./quadrature-replay --out ./quadrature-replay.json --blocks 24
```
