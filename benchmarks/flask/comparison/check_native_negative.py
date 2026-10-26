"""Check that a real native HTTP control cannot produce an Imre benchmark row."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust", required=True, type=Path)
    parser.add_argument("--imre-binary", type=Path, help="alternative native control that serves the Flask health response")
    args, comparison_options = parser.parse_known_args()
    with tempfile.TemporaryDirectory() as temp:
        output = Path(temp) / "comparison.json"
        subprocess.run([sys.executable, HERE / "compare.py", *comparison_options, "--rust", args.rust,
                        "--imre-binary", args.imre_binary or args.rust, "--output", output,
                        "--rounds", "1", "--mixed-only", "--concurrency", "1", "--warmup", "9",
                        "--requests", "9"], check=True)
        report = json.loads(output.read_text())
        proof = report["imre_native_preflight"]
        expected = json.loads((HERE.parent / "expected.json").read_text())["loopback_tcp"]
        assert proof["native_identity"]["binary_sha256"] == report["executables"]["imre_input"]["sha256"]
        assert proof["responses"]["health"] == expected["health"]
        if not args.imre_binary:
            assert proof["responses"]["bigint"] == expected["bigint"]
        assert proof["outcome"] != "pass" and proof["verified_native_responses"] == 0
        assert {row["server"] for row in report["trials"]} == {"cpython", "node", "bun", "rust"}
        print("The native HTTP control was rejected. All four comparison controls completed the smoke round.")


if __name__ == "__main__":
    main()
