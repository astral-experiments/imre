# Frozen Flask, stock CPython Flask, Node, Bun and Rust

The [existing comparison harness](../comparison/compare.py) accepts the frozen O3 executable as its Imre candidate. It stages the seven pinned wheels afresh for stock Flask on CPython and checks their hashes. Node and Bun run the same [JavaScript control](../comparison/control.mjs). Rust runs the [locked Hyper implementation](../comparison/rust). All five receive identical requests from the Go client.

Install Node and Bun on `PATH` alongside the [frozen benchmark prerequisites](README.md). The recorded run used Node 24.22.1 and Bun 1.4.2. From the checkout, run the five-server comparison.

```sh
python3.14 benchmarks/flask/frozen/run.py --five-servers
```

Each server runs three trials with 180 warmups and 1,800 measured requests per trial at concurrency eight. Add `--smoke` to run one trial with nine warmups and 18 measured requests per server. Use `--server-cpu` and `--client-cpus` to select CPU affinity.

The command verifies that the built frozen O3 executable passes ten traced HTTP checks and requires a complete timed trial for every server in every round. It prints the path to `results.json`, which contains versions, hashes, per-route latency, per-round samples, and socket ownership evidence. The result labels are `imre` for the frozen executable and `cpython` for stock Flask.

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
