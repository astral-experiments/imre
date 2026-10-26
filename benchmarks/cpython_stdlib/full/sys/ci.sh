#!/usr/bin/env bash
set -euo pipefail
export LANG=C.UTF-8 LC_ALL=C.UTF-8 LC_CTYPE=C.UTF-8
unset PYTHONUTF8 PYTHONIOENCODING
sys_runner=(benchmarks/cpython_stdlib/full/sys/run.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference)
python3 benchmarks/cpython_stdlib/native_sys.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18
if [[ -f benchmarks/cpython_stdlib/full/sys/codec_device.py ]]; then
  python3 -B benchmarks/cpython_stdlib/full/sys/codec_device.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference --result "$RUNNER_TEMP/sys-codec-device.json"
  cmp benchmarks/cpython_stdlib/full/sys/codec-device-linux.json "$RUNNER_TEMP/sys-codec-device.json"
fi
if [[ -f benchmarks/cpython_stdlib/full/sys/stderr_encoding.py ]]; then
  python3 -B benchmarks/cpython_stdlib/full/sys/stderr_encoding.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference --result "$RUNNER_TEMP/sys-stderr-encoding.json"
fi
if [[ -f benchmarks/cpython_stdlib/full/sys/stdout_encoding.py ]]; then
  python3 -B benchmarks/cpython_stdlib/full/sys/stdout_encoding.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference --result "$RUNNER_TEMP/sys-stdout-encoding.json"
fi
if [[ -f benchmarks/cpython_stdlib/full/sys/stdin_encoding.py ]]; then
  python3 -B benchmarks/cpython_stdlib/full/sys/stdin_encoding.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --cpython-source .cpython-stdlib-reference --result "$RUNNER_TEMP/sys-stdin-encoding.json"
  cmp benchmarks/cpython_stdlib/full/sys/stdin-encoding-linux.json "$RUNNER_TEMP/sys-stdin-encoding.json"
fi
for opt in 0 3; do
  python3 -B "${sys_runner[@]}" --opt-level "$opt" --result "$RUNNER_TEMP/test-sys-O$opt.json" --evidence "$RUNNER_TEMP/sys-evidence-O$opt.json"
  python3 -B -X utf8 "${sys_runner[@]}" --opt-level "$opt" --locale-audit "$RUNNER_TEMP/sys-locale-utf8-O$opt.json"
  python3 benchmarks/cpython_stdlib/full/sys/argv_boundary.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --opt-level "$opt"
done
jq -s . "$RUNNER_TEMP/sys-evidence-O0.json" "$RUNNER_TEMP/sys-evidence-O3.json" > "$RUNNER_TEMP/sys-evidence.json"
python3 benchmarks/cpython_stdlib/suite_evidence.py benchmarks/cpython_stdlib/full/sys/reference-linux.json "$RUNNER_TEMP/sys-evidence.json"
python3 benchmarks/cpython_stdlib/suite_evidence.py benchmarks/cpython_stdlib/full/sys/reference-linux.json benchmarks/cpython_stdlib/full/sys/evidence-linux.json
git rev-parse HEAD 'HEAD^{tree}' > "$RUNNER_TEMP/sys-source.txt"
