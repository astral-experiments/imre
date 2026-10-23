# Ruff’s live memory report

Ruff’s original `run` command invokes ty 0.0.89 and 0.0.90 on flake8, Sphinx, Prefect, and Trio. Every execution writes eight new JSON captures and renders its own Markdown. These timings include the external tools. The [file comparison benchmark](2026-10-23-original-memory-report.md) times the same reporter on frozen input.

| Four prepared projects | Median wall time |
| --- | ---: |
| Relar | 34.04 s |
| CPython 3.14.5 | 34.38 s |

Individual runs ranged from 33.85 to 36.42 s for Relar and 34.07 to 34.56 s for CPython.

Each engine had one warmup and four measured runs. The two engines alternated first position and ran with the same two-CPU affinity. Both used the same project virtual environments, uv 0.13.5, and ty binaries. The measurements exclude AOT compilation and include Ruff’s locked setup helper calls, the eight analyzer processes, and report publication. [Ruff script](https://github.com/astral-sh/ruff/blob/601192dfdb7ac4a636dec0d7e2daf14ba179cfc8/scripts/memory_report.py) · [Runner](../memory_program_live.py)

The validator checked the four project names, eight captures, progress on stderr, empty stdout, and the echoed output file. It also ran CPython’s `compare` on each invocation’s own captures because ty’s allocator can produce different totals on successive runs. 8 of the eight measured reports matched those bytes exactly. The [raw measurements](2026-10-23-original-memory-live.json.gz) include every timing, the project commits, and the hashes of all eight captures from each measured run.

A separate Relar execution started with an empty project directory and a populated uv download cache. Cloning the four projects, installing their dependencies, running both ty versions, and writing the report took **56.36 s**. Its 5,759-byte Markdown file matched CPython’s output for those captures (SHA-256 `8cda2e4dc4a2be53524e5c7497943f7206fbf976df463ddffe8ab066f13602e3`).

## Reproduce

Use a Ruff checkout containing the setup script and its lockfile. Build once before timing. The first invocation prepares the four projects. Use absolute paths for `OLD_TY` and `NEW_TY`. [Ruff setup helper](https://github.com/astral-sh/ruff/blob/601192dfdb7ac4a636dec0d7e2daf14ba179cfc8/scripts/setup_primer_project.py)

```sh
"$RELAR" build "$RUFF/scripts/memory_report.py" --whole-program --opt-level 3 \
  --runtime "$THIN_RUNTIME" --target x86_64-unknown-linux-gnu \
  --lto-linker "$LTO_LINKER" -o "$BUILD/memory-report"
RELAR_PYTHON="$CPYTHON" "$BUILD/memory-report" run --projects-dir "$BUILD/projects" \
  --old-ty "$OLD_TY" --new-ty "$NEW_TY" --output "$BUILD/report.md"
taskset -c 8,9 "$CPYTHON" benchmarks/memory_program_live.py \
  --source "$RUFF/scripts/memory_report.py" --relar "$BUILD/memory-report" \
  --projects-dir "$BUILD/projects" --old-ty "$OLD_TY" --new-ty "$NEW_TY" \
  --samples 4 --output "$BUILD/live.json.gz"
```
