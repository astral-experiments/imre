# Publisher cargo fixture policy v1

This supplemental policy applies only to the cargo-stub subprocess in the frozen
`ruff-publisher` and `uv-publisher` cases at stock commit
`99964c6734dbe008d69d5016eb60d25abd31ab0d`. It grants no workload, general
Python, Rich, ty, historical-cohort, or blanket stdlib allowance. The executable,
stub, accepted argv tails, and each relative bytecode path and SHA256 are fixed
in `policy.json`. Resolve the stdlib root from the trusted pinned executable's
directory, not from a receipt.

The frozen stub imports `json`, `os`, `pathlib`, and `sys`. The 25 bytecode
files fall into these legitimate groups:

- `encodings/{__init__,aliases,utf_8}` loads during CPython startup.
- `json/{__init__,decoder,encoder,scanner}` implements the stub's metadata and
  journal JSON.
- `pathlib/{__init__,_os}` implements the stub's publish-marker path.
- The exact `re`, `collections`, and top-level modules enumerated in the policy
  are transitive stdlib imports of startup and those explicit imports. `os` and
  `sys` need no extra bytecode allowance in this interpreter.

The independent fixture-only control supplied for this freeze ran the pinned
interpreter and stub with the metadata argv under the frozen environment. It
exited 0; its successful sensitive stdlib bytecode opens were exactly the 25
listed paths. Every file had magic `2b0e0d0a`, flags 0, and timestamp/source-size
header fields matching its adjacent CPython stdlib `.py` source. Four older
publisher traces agreed, but were not used to define the policy. In this
authoring workspace the stock stub hash was independently verified; the pinned
interpreter was unavailable, so the fixture-only execution and bytecode/header
checks could not be repeated here.

To reproduce only that control, first obtain the exact pinned executable and the
stub from the stock commit and verify their policy SHA256 values. Set `PYTHON`
and `STUB` to those trusted paths, then run in a disposable directory:

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

Compare successful sensitive bytecode opens against the exact policy set and
hash each opened file. Inspect each 16-byte pyc header and compare its little
endian timestamp and size fields to the adjacent source's mtime and size.

A prospective validator must anchor the independently reviewed public policy
commit, blob, and SHA256 after merge. It must reject unlisted cases, processes,
paths, rebound paths, mismatched hashes, and injected payloads. Receipt-supplied
policy content, roots, lists, or hashes cannot add permissions or replace those
anchors; the negative controls belong in that separate validator change.
