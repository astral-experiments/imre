"""Compare Imre's read-only patch objects with unidiff 1.0.0."""

import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

SOURCE = Path(__file__).parent / "fixtures/patchset/inspect_patch.py"
CASES = {
    "empty": "",
    "multiple": (
        "diff --git a/a.py b/a.py\nindex aaa..bbb 100644\n--- a/a.py\n+++ b/a.py\n"
        "@@ -01 +0002 @@ function()\n-before\n+after\n"
        "@@ -4,2 +5,2 @@\n\n-leading ☃\n+trailing 🐍\n\\ No newline at end of file"
    ),
    "new-removed": (
        "diff --git a/new b/new\nnew file mode 100644\n--- /dev/null\n+++ b/new\n"
        "@@ -0,0 +1 @@\n+new\n"
        "diff --git a/gone b/gone\ndeleted file mode 100644\n--- a/gone\n+++ /dev/null\n"
        "@@ -1 +0,0 @@\n-gone\n"
        "diff --git a/empty b/empty\nnew file mode 100644\n"
    ),
    "git-metadata": (
        'diff --git "a/old name" "b/new name"\nsimilarity index 100%\n'
        'rename from old name\nrename to new name\n'
        "diff --git a/bin b/bin\nindex aaa..bbb 100644\nBinary files a/bin and b/bin differ\n"
        "diff --git a/tool b/tool\nold mode 100644\nnew mode 100755\n"
    ),
    "timestamps": "--- old\t2026\n+++ new\t2027\n@@ -1 +1 @@\n-old\r\n+new",
    "normalization": (
        "--- a/file\n+++ b/file\n@@ -1,2 +3,2 @@  docs\n un├icode\n"
        "-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n\n"
    ),
    "large-start": "--- a/x\n+++ b/x\n@@ -1000000000000000000001 +1 @@\n hi\n",
    "unicode-numbers": "--- a/x\n+++ b/x\n@@ -١,２ +0𝟑,１ @@ ٠ remains in the section\n-hey\n hi\n",
    "short": "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n line\n",
    "long": "--- a/x\n+++ b/x\n@@ -1,0 +1 @@\n line\n",
    "bad-body": "--- a/x\n+++ b/x\n@@ -1 +1 @@\nwrong\n",
    "unexpected-hunk": "@@ -1,2 +1,2 @@\n a\n b\n",
    "unexpected-marker": "\\ No newline at end of file\n",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        directory = Path(tmp)
        cases = []
        for name, text in CASES.items():
            sample = directory / f"{name}.patch"
            sample.write_bytes(text.encode())
            cases.append((name, SOURCE, [sample]))
        cases.append(("exception-identity", SOURCE.with_name("exception_identity.py"), []))
        for level in ("0", "3"):
            programs = {}
            for name, source, argv in cases:
                if source not in programs:
                    binary = directory / f"{source.stem}-{level}"
                    subprocess.run(
                        [args.compiler.resolve(), "build", source, "--runtime",
                         args.runtime.resolve(), "--opt-level", level, "-o", binary], check=True,
                    )
                    programs[source] = binary
                reference = subprocess.run([args.python, source, *argv], capture_output=True, check=True)
                actual = subprocess.run([programs[source], *argv], capture_output=True, check=True)
                assert (reference.stdout, reference.stderr) == (actual.stdout, actual.stderr), (
                    name, level, reference, actual,
                )
    print(f"{len(CASES)} unified diff cases and distinct exception handlers matched at O0/O3")


if __name__ == "__main__":
    main()
