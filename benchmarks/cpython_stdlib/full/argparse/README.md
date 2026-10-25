# CPython argparse

The denominator is **1,893 distinct unittest IDs** from CPython 3.14.5
(`5607950ef232dad16d75c0cf53101d9649d89115`). The release has one
`test_argparse.py` module. Its metaclasses generate test methods, so counting
source definitions would miss cases. The vendored files include the translation
snapshot used by `TestTranslations.test_translations` and the upstream license.

The test-enabled release interpreter on Linux passed all 1,893 IDs with no skips.
`reference-linux.json` records one result per ID; reference success is not native
coverage. To reproduce with the shared inventory runner:

```sh
python3.14 -B benchmarks/cpython_stdlib/suite_inventory.py \
  benchmarks/cpython_stdlib/full/argparse/UPSTREAM.json \
  --result benchmarks/cpython_stdlib/full/argparse/reference-linux.json
```

Existing native CLI differential fixtures compare exit status, stdout and stderr
with CPython. They are regression checks, not executions of these unittest IDs.

[Native routine results](NATIVE.md) report which original routines drove Imre
parser operations, what ran on the host, and which IDs are still blocked.
