# Publisher cargo fixture policy v1

This supplemental policy covers only the frozen cargo-stub subprocess for
`ruff-publisher` and `uv-publisher` at stock commit
`99964c6734dbe008d69d5016eb60d25abd31ab0d`. `policy.json` fixes the executable
and stub hashes, both accepted argv tails, and exactly 25 relative CPython
stdlib source paths and hashes. It grants no general Python or stdlib exception.

The profile uses a private hardlink copy of the pinned CPython 3.14.5
distribution, excluding every `__pycache__` directory and leaving the shared
toolchain unchanged. The effective executable retains the pinned SHA256, and
`sys.prefix` must equal the private distribution root. Normalize the stdlib root
as `resolve(dirname(effective_executable) / "../lib/python3.14")`, using the
independently pinned effective executable directory, never a receipt-selected
root. Public prospective activation must independently bind the effective
private executable path, prefix, and file identities as well as their hashes.
The frozen `PYTHONDONTWRITEBYTECODE=1` environment entry is unchanged.

The stub imports `json`, `os`, `pathlib`, and `sys`. The three `encodings`
sources support interpreter bootstrap; the four `json` sources implement
metadata and journal JSON; the two `pathlib` sources support the publish-marker
path. The enumerated `re`, `collections`, and top-level sources are transitive
stdlib imports. `os` and `sys` require no additional source allowance.

Prior native traces initially suggested the candidate list. Authority comes from
the coordinator's independent, isolated fixture-only control, not from native
outcome hindsight: the private pinned interpreter ran the exact frozen stub and
metadata argv with the original frozen environment and unchanged stock subtree.
It exited 0, produced byte-exact stock metadata stdout, empty stderr, and one
cargo-journal row. Its successful sensitive opens were precisely the 25 listed
`.py` sources, with matching hashes and zero bytecode opens. This branch update
did not rerun that control.

To reproduce the fixture-only control, prepare the private copy excluding caches,
verify the effective executable hash and prefix, and verify the stock stub hash.
Set `PYTHON` to that private executable and `STUB` to the immutable stock stub:

```sh
work=$(mktemp -d)
mkdir "$work/home"
(
  cd "$work" || exit 1
  umask 0077
  env -i CARGO_LOG="$work/cargo.log" HOME="$work/home" LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 NO_PROXY=127.0.0.1 PATH=/usr/bin \
    PYTHONDONTWRITEBYTECODE=1 PYTHONHASHSEED=0 PYTHONNOUSERSITE=1 TZ=UTC \
    /usr/bin/strace -f -qq -e trace=open,openat,openat2 -o "$work/opens" \
    "$PYTHON" "$STUB" metadata --format-version 1 --no-deps
)
```

Compare the output and journal to stock and the successful sensitive opens to
the exact source manifest; reject any bytecode open. A separate prospective
validator must anchor the reviewed public policy commit, blob, and SHA256 after
merge, reject rebound paths, hash mismatches, and injected payloads, and never
accept receipt-supplied roots, policy content, or permissions as authority.
