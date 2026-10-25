# JSON formatting fixtures

Run the unchanged formatter cases with the pinned CPython release and the native compiler at both optimization levels:

```sh
python -B benchmarks/cpython_stdlib/full/json/formatting.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result /tmp/json-formatting.json
```

The [native formatting report](formatting-linux.json) covers five distinct source methods across nine fixture IDs and 33 native operations per level. The original methods call the native standard `json.dumps` and `json.loads` on the actual fixture inputs, including mixed nested containers. Operand transport uses Python literals and never calls CPython's JSON implementation. CPython supplies the fixture lifecycle, test bodies and assertions. Each recorded operation names its method, native source SHA-256, and result or error category.
