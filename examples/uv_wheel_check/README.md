# uv wheel contents checker

[uv's release script](https://github.com/astral-sh/uv/blob/93a96e916298e7fba21e6212b29a3474e0e268c5/scripts/check_uv_wheel_contents.py)
is compiled verbatim. It opens each wheel, normalizes its version and Windows
executable names, and compares the member names against the expected files.
The source and both upstream licenses are pinned in [the manifest](upstream/UPSTREAM.json).

Run the same checker on CPython and Imre:

```sh
python3 tests/uv_wheel_check.py --compiler target/release/imre \
  --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18
```

The gate builds at O0 and O3 and creates tiny ZIP fixtures locally. To also check
release artifacts, append `--wheels /path/to/uv-….whl /path/to/uv_build-….whl`.
No Python packages or network downloads are needed.
