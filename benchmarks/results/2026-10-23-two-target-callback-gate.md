# Two-target loop specialization on the merged runtime

The capped direct LLVM prototype failed the 1% slowdown limit for repeated selective gross payouts on both tested CPUs. We compared the merge commit `bd83e99` with its actual parent `8184ed5` using the same freshly built runtime and ordinary split-module builds. An intervening runtime-startup change had changed the final executables after the original PR measurements. [Prototype and original study](https://github.com/astral-sh/relar/pull/146) · [Landed samples, build hashes and outputs](2026-10-23-two-target-callback-gate.json.gz)

Each payout process parsed 4,096 records and ran 256 queries. The full filter passed all records and the selective filter passed 403. The table gives median paired speedups, computed as parent time divided by prototype time, with 95% bootstrap intervals.

| Payout | CPU 20 | CPU 12 |
| --- | ---: | ---: |
| Full gross | 1.0547× [1.0492, 1.0599] | 1.0560× [1.0505, 1.0599] |
| Full net | 1.0582× [1.0546, 1.0616] | 1.0580× [1.0533, 1.0626] |
| Selective gross | **0.9743× [0.9704, 0.9789]** | **0.9749× [0.9712, 0.9789]** |
| Selective net | 1.0097× [1.0070, 1.0142] | 1.0083× [1.0050, 1.0116] |

The one-query cases all passed. Each CPU ran 160 balanced paired blocks per case. All 5,120 timed processes and both compilers’ 122 payout-oracle cases matched the expected status, stdout, stderr and files. The failing workload made 256 target checks and 103,168 direct gross calls in the prototype. The parent made 103,168 indirect gross calls. The native summary grew by 761 bytes and executable `.text` by 768 bytes.

The second caller was TheAlgorithms/Python’s trapezoidal integrator. In 24 paired blocks on CPU 3, the landed build sped up the constant integrand by 1.1154× [1.1022, 1.1223]. The two quadratic intervals sped up by 1.0583× [1.0476, 1.0654] and 1.0648× [1.0583, 1.0882]. With the callbacks in another module, the entire executable matched the parent byte-for-byte. Earlier linked builds showed a 1.1–1.25% slowdown on the same quadratic inputs. The integration function came directly from upstream. The driver used `5.0` and `9 * (x * x)` to represent the published callbacks within Relar’s supported float syntax. All final integrals and repeated checksums matched CPython bit-for-bit.

In a separate layout-only pilot on CPU 8, the original matched binaries gave a selective gross speedup of 0.966× [0.952, 0.979]. Placing 32 unexecuted bytes immediately before the summary function in both binaries changed it to 1.016× [1.002, 1.028]. A different shift hurt selective net. Native code placement can change the sign of this comparison. The pilot does not identify a specific microarchitectural cause or a placement rule that preserves the gate across builds.

## Reproduction

Use separate checkouts of the two revisions above, the same release runtime, and a matching Linux Clang/LLD 18 bundle next to each release compiler. Use the payout schedule and drivers from the prototype commit. The payout driver accepts the two native applications and checks the oracle before replaying the schedule. The integration driver accepts the two compilers and runtime and checks both module layouts. [Payout recipe](https://github.com/astral-sh/relar/blob/bd83e99269abd0c8b1e61a32cf8fab826a1017e8/benchmarks/results/2026-10-23-payout-callbacks.md) · [Integration recipe](https://github.com/astral-sh/relar/blob/bd83e99269abd0c8b1e61a32cf8fab826a1017e8/benchmarks/callback_integration/README.md)
