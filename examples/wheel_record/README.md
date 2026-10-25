# Wheel RECORD validation

`validate.py` opens a wheel with `zipfile.ZipFile` and calls PyPA installer's publicly exposed `WheelFile.validate_record()`. The files in `upstream/installer/` and the license are the original blobs from [installer at 9b7fd286](https://github.com/pypa/installer/tree/9b7fd286b85c5091390d4368709c46b35693b28c). The build evaluates the reachable source; the test does not substitute a local RECORD parser or verifier.

Run the oracle and native comparison with:

```sh
python3 tests/wheel_record.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang clang --python /path/to/python3.14
```

Supply `--real-wheel /path/to/pip-...whl` to check a published wheel, including re-packing it after changing one byte without changing the member's length. The altered archive must pass its ZIP CRC check and fail its RECORD check. Every run also checks stored/deflated members, quoted filenames, wrong sizes and missing records. Error comparison excludes traceback formatting and checks the exception class, complete text, status and stdout. `--reference-only` exercises CPython without claiming a native result.
