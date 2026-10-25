# Codec lookup in CPython's OS suite

CPython 3.14.5 imports `codecs` at [test_os.py:6](upstream/Lib/test/test_os.py#L6).
Its only use in the suite is [DeviceEncodingTests.test_device_encoding](upstream/Lib/test/test_os.py#L3554):
it obtains a platform encoding from `os.device_encoding(0)` and asserts that
`codecs.lookup(encoding)` is truthy. The POSIX and genericpath companions and
the inherited environment mapping tests have no direct codec lookup.

On [the shared registry in #691](https://github.com/astral-sh/imre/pull/691)
at [c53dc9fc](https://github.com/astral-sh/imre/commit/c53dc9fcdbf1a09339cbe62f5971d5147ff88a86),
all four unchanged [grouped entry checks](observations/c53dc9fc/whole-suite-startup-linux.json)
advance past `import codecs` and stop on `import decimal` at line 8. The same
four entries also fail at that import when built at O0 and O3. The only use of
`decimal` is [Decimal(-1) in the chown argument test](upstream/Lib/test/test_os.py#L2030).
**No whole-suite occurrence executed: 0 of 579 at each optimization level.**

The [independent entry checks](observations/c53dc9fc/suite-startup-linux.json)
expose the companion failures that the first import in the grouped entry masks:

| Suite | Linux occurrences | First diagnostic on the shared registry |
| --- | ---: | --- |
| test_os | 375 | Line 8: unsupported `decimal` import |
| test_posix | 179 | Line 1845: the starred tuple element requires a bounded, unmodified sequence |
| test_genericpath | 25 | Line 5: unsupported `genericpath` import; inventory only |

The OS comparison reruns [the registry owner's fixture](https://github.com/astral-sh/imre/blob/c53dc9fcdbf1a09339cbe62f5971d5147ff88a86/benchmarks/cpython_stdlib/full/sys/codec_device.py)
against this exact compiler. In a PTY, CPython runs the original fixture, body,
assertion, and `os.device_encoding`; Imre resolves `UTF-8` to the `utf-8` entry
and evaluates its truth. With redirected stdin the original method skips and
no lookup runs. The [O0/O3 results](observations/c53dc9fc/shared-codec-consumer-linux.json)
are byte-identical to the registry owner's release receipt. This observation
adds no native OS operation or additional suite occurrence.

## Reproduce the grouped checks

Use an Imre compiler and runtime built from the revision above, a CPython Git
checkout containing the pinned release, and a temporary directory for the
original release sources. Run from the Imre repository root:

```sh
pin=5607950ef232dad16d75c0cf53101d9649d89115
work=$(mktemp -d)
git -C "$CPYTHON_GIT" archive "$pin" Lib/test LICENSE | tar -x -C "$work"
cp benchmarks/cpython_stdlib/full/os/entries/*.py "$work/Lib/"
for entry in imre_os_imports imre_os_run; do
    for scope in all reachable; do
        set --
        if [ "$scope" = reachable ]; then set -- --reachable-only; fi
        "$IMRE" check "$work/Lib/$entry.py" --module-root "$work/Lib" "$@"
        for level in 0 3; do
            "$IMRE" build "$work/Lib/$entry.py" --module-root "$work/Lib" "$@" \
                --opt-level "$level" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
                --no-cache --quiet -o "$work/program"
        done
    done
done
```

For the separate suites, run [check_suite_startup.py](../check_suite_startup.py)
with `--compiler "$IMRE" --cpython-git "$CPYTHON_GIT" test.test_os test.test_posix test.test_genericpath`.
The [provenance](observations/c53dc9fc/registry-provenance-linux.json) records the
release test tree, individual blobs and license, entry scripts, compiler, runtime,
and report hashes.

The [aaf044ff report](observations/aaf044ff/registry-provenance-linux.json)
and [624d69d6 report](observations/624d69d6/registry-provenance-linux.json)
retain the earlier compiler and runtime hashes. Integrating the registry with
native `ChainMap` at `c53dc9fc` changed neither the first diagnostics nor the
consumer report. All three runs used the same unchanged suite entries.
