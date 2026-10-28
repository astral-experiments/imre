# Frozen Flask, stock CPython Flask, Node, Bun and Rust

The [existing comparison harness](../comparison/compare.py) accepts the frozen O3 executable as its Imre candidate. It stages the seven pinned wheels afresh for stock Flask on CPython and checks their hashes. Node and Bun run the same [JavaScript control](../comparison/control.mjs). Rust runs the [locked Hyper implementation](../comparison/rust). All five receive identical requests from the Go client.

Install Node and Bun on `PATH` in addition to the [frozen benchmark prerequisites](README.md). The recorded run used Node 24.22.1 and Bun 1.4.2. Run from the checkout or set `IMRE_CHECKOUT` to its path. Select one available server CPU and three distinct client CPUs.

```sh
set -eu
repo=$(git -C "${IMRE_CHECKOUT:-.}" rev-parse --show-toplevel)
work=$(mktemp -d)/frozen
server_cpu=${SERVER_CPU:-2}
client_cpus=${CLIENT_CPUS:-3,4,5}

python3.14 "$repo/benchmarks/flask/frozen/run.py" --smoke --work "$work" \
    --server-cpu "$server_cpu" --client-cpus "$client_cpus"

FLASK_SECRET_KEY=local-probe-key python3.14 "$repo/benchmarks/flask/comparison/compare.py" \
    --wheelhouse "$work/build/wheels" --imre-binary "$work/build/mixed/server-O3" \
    --client "$work/client" --rust "${CARGO_TARGET_DIR:-$repo/target}/release/imre-flask-hyper-control" \
    --node "$(command -v node)" --bun "$(command -v bun)" \
    --server-cpu "$server_cpu" --client-cpus "$client_cpus" --concurrency 8 \
    --mixed-only --rounds 3 --warmup 180 --requests 1800 --output "$work/five.json"

python3.14 - "$work/five.json" "$work/build/build.json" <<'PY'
import json, sys
from collections import Counter
from pathlib import Path
report, build = [json.loads(Path(p).read_bytes()) for p in sys.argv[1:]]
names = {"imre", "cpython", "node", "bun", "rust"}
assert report["imre_native_preflight"]["verified_native_responses"] == 10
assert report["imre_build"]["binary_sha256"] == build["graphs"]["mixed"]["builds"]["3"]["elf_sha256"]
expected = Counter((name, n) for name in names for n in (1, 2, 3))
assert Counter((t["server"], t["round"]) for t in report["trials"]) == expected
assert all(t["All"]["Count"] == t["Requests"] == 1800 for t in report["trials"])
print("All five servers passed, with 27,000 validated timed responses.")
PY
```

The harness labels the frozen executable `imre` and stock Flask `cpython`. The last check requires timed responses from every server because the harness can continue timing controls after an Imre admission failure. The JSON contains individual samples, versions, hashes, per-route latencies, and the native process and socket checks.

## Recorded comparison

Each server completed three mixed-workload trials of 1,800 requests on one Milan core. The client used three separate cores and checked all 27,000 responses. The table reports medians across trials. [Per-server and per-route samples](five-server-results.json)

| Server | Requests/s | Trial p95 |
| --- | ---: | ---: |
| Frozen Flask on Imre | 9,513 | 1.410 ms |
| Rust / Hyper | 8,831 | 1.699 ms |
| Bun | 7,224 | 1.697 ms |
| Node | 2,494 | 5.213 ms |
| Stock Flask on CPython | 942 | 9.932 ms |

The [longer frozen/Rust comparison](README.md#recorded-results) used 36,000 requests per server per trial.
