#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C.UTF-8 PYTHONDONTWRITEBYTECODE=1
b="$RUNNER_TEMP/textio-controls"
cd "$GITHUB_WORKSPACE"
test "$(git rev-parse HEAD)" = "$SYS_SOURCE"
test "$(git -C .cpython-stdlib-reference rev-parse HEAD)" = 5607950ef232dad16d75c0cf53101d9649d89115
sha256sum target/release/imre target/release/libimre_runtime.a > "$b/verify-ir-products.log"
cargo build --release --locked -p imre-compiler -p imre-runtime -j 2
python3 -B "$b/prepare_run.py"
oracle=$(command -v python3)
set +e
python3 -B "$b/direct_controls.py" "$b" "$b/area" "$oracle" > "$b/click.log" 2>&1
click=$?
set -e
for driver in original_io.py original_io_lifetime.py original_io_chunk_v2.py registered.py; do
    python3 -B "$b/$driver" "$SYS_SOURCE" > "$b/$driver.log" 2>&1 || { tail -30 "$b/$driver.log"; exit 1; }
done
python3 -B "$b/summarize.py"
test "$click" = 0
python3 -B tests/python_cli.py --compiler target/release/imre > "$b/python-cli.log" 2>&1 || { tail -30 "$b/python-cli.log"; exit 1; }
echo "Complete original CLI passed" >> "$b/python-cli.log"
