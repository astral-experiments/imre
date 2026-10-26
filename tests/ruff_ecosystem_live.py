"""Run both commands of one compiled Ruff ecosystem CLI against CPython."""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "ruff_ecosystem"
PROJECTS = ["changed", "unchanged", "failed"]
CONFIG = b'# original fixture\nrequired-version = "==0.0.1"\n'

# Upstream's CLI only exposes its default project list. Replace that data module
# in the temporary package so the upstream entrypoint and all functions stay exact.
DEFAULTS = """from ruff_ecosystem.projects import Project, Repository

DEFAULT_TARGETS = [
    Project(Repository("fixture", "changed", "main")),
    Project(Repository("fixture", "unchanged", "main")),
    Project(Repository("fixture", "failed", "main")),
]
"""

# Protocol errors, stderr, config patching and diagnostics come from actual
# child processes. The large benchmark separately runs real Ruff releases.
LINTER = '''#!/bin/sh
set -eu
if [ "$1" = rule ]; then
    [ "$ECOSYSTEM_CASE" != preview ]
    [ "$*" = "rule --all --output-format json" ]
    printf '%s\\n' '[{"name":"unused-import","code":"F401"}]'
    exit
fi
if [ "$1" = format ]; then
    [ "$(cat revision.txt)" = tip ]
    [ "$(cat sample.py)" = original ] || { echo "formatter did not start from the original revision" >&2; exit 90; }
    ! grep -q required-version ruff.toml || exit 80
    project=${PWD##*:}
    version=${0##*/}
    if [ "$ECOSYSTEM_CASE:$project:$version" = format-error:failed:comparison ]; then
        echo "synthetic formatter failed" >&2
        exit 17
    fi
    if [ "$ECOSYSTEM_CASE" != format-unchanged ] && [ "$project" = changed ]; then
        echo "$version" > sample.py
    fi
    exit 0
fi
[ "$1" = check ]
case " $* " in
    *" --preview "*) [ "$ECOSYSTEM_CASE" = preview ];;
    *) [ "$ECOSYSTEM_CASE" != preview ];;
esac
[ "$(cat revision.txt)" = tip ]
if grep -q required-version ruff.toml; then
    echo "configuration was not patched" >&2
    exit 80
fi
project=${PWD##*:}
version=${0##*/}
if [ "$ECOSYSTEM_CASE" = checker-error ] && [ "$project:$version" = failed:comparison ]; then
    echo "synthetic checker failed" >&2
    exit 17
fi
if [ "$ECOSYSTEM_CASE" != unchanged ] && [ "$project:$version" = changed:comparison ]; then
    echo "sample.py:2:3: F401 [*] unused import"
fi
echo "sample.py:1:1: F821 unknown name"
echo "Found 1 error."
'''


def git(directory, *arguments):
    result = subprocess.run(
        ["git", "-C", str(directory), *map(str, arguments)],
        check=True, capture_output=True, text=True, timeout=20,
    )
    return result.stdout.strip()


def seed(directory):
    """Seed two independent caches before advancing the same disposable remote."""
    source = directory / "source"
    source.mkdir()
    git(source, "init", "-q", "-b", "main")
    git(source, "config", "user.name", "Imre tests")
    git(source, "config", "user.email", "tests@example.invalid")
    git(source, "config", "commit.gpgSign", "false")
    (source / "ruff.toml").write_bytes(CONFIG)
    (source / "sample.py").write_text("original\n")
    (source / "revision.txt").write_text("base\n")
    git(source, "add", ".")
    git(source, "commit", "-qm", "base")
    remote = directory / "remote.git"
    git(directory, "clone", "-q", "--bare", str(source), str(remote))
    caches = [directory / name for name in ["cpython-cache", "imre-cache"]]
    for cache in caches:
        cache.mkdir()
        for project in PROJECTS:
            checkout = cache / f"fixture:{project}"
            git(directory, "clone", "-q", str(remote), str(checkout))
            git(checkout, "config", "user.name", "Ecosystem Bot")
            git(checkout, "config", "user.email", "ecosystem@astral.sh")
            git(checkout, "config", "commit.gpgSign", "false")
    (source / "revision.txt").write_text("tip\n")
    git(source, "commit", "-qam", "tip")
    git(source, "push", "-q", str(remote), "main")
    return caches, git(source, "rev-parse", "HEAD")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--opt-level", choices=["0", "3"], default="3")
    args = parser.parse_args()
    manifest = json.loads((EXAMPLE / "UPSTREAM.json").read_text())
    for name, digest in manifest["files"].items():
        assert hashlib.sha256((EXAMPLE / name).read_bytes()).hexdigest() == digest, name
    with tempfile.TemporaryDirectory(prefix="imre-ecosystem-live-") as temporary:
        directory = Path(temporary)
        package = directory / "ruff_ecosystem"
        shutil.copytree(EXAMPLE / package.name, package, ignore=shutil.ignore_patterns("__pycache__"))
        (package / "defaults.py").write_text(DEFAULTS)
        binary = directory / "ruff-ecosystem"
        subprocess.run(
            [args.compiler.resolve(), "build", package / "__main__.py", "--module-root", directory,
             "--reachable-only", "--entry-profile", EXAMPLE / "markdown-entry.json",
             "--opt-level", args.opt_level, "--runtime", args.runtime.resolve(), "-o", binary],
            check=True, timeout=300,
        )
        executables = [directory / name for name in ["baseline", "comparison"]]
        for executable in executables:
            executable.write_text(LINTER)
            executable.chmod(0o755)
        caches, tip = seed(directory)
        commands = [[args.python, "-m", "ruff_ecosystem"], [binary]]
        for case in ["unchanged", "changed", "preview", "checker-error",
                     "format-unchanged", "format-changed", "format-error", "git-error"]:
            subcommand = "format" if case.startswith("format-") else "check"
            if case == "git-error":
                for cache in caches:
                    git(cache / "fixture:failed", "remote", "set-url", "origin", directory / "missing.git")
            output = []
            for cache, command in zip(caches, commands, strict=True):
                for project in PROJECTS:
                    (cache / f"fixture:{project}" / "ruff.toml").write_text("# dirty checkout\n")
                options = ["--force-preview"] if case == "preview" else []
                result = subprocess.run(
                    [*command, "--cache", cache, *options, subcommand, *executables],
                    cwd=directory, env={**os.environ, "PYTHONPATH": str(directory), "ECOSYSTEM_CASE": case},
                    capture_output=True, timeout=30,
                )
                assert result.returncode == 0, (case, command, result.stderr)
                for project in PROJECTS:
                    checkout = cache / f"fixture:{project}"
                    assert (checkout / "ruff.toml").read_bytes() == CONFIG, (case, project)
                    assert git(checkout, "rev-parse", "HEAD") == tip
                output.append((result.stdout, result.stderr))
            assert output[0] == output[1], (case, output)
            stdout, stderr = output[0]
            assert not stderr, (case, stderr)
            if case == "format-unchanged":
                assert stdout.decode() == "✅ ecosystem check detected no format changes.\n"
            elif subcommand == "format":
                assert b"-baseline\n+comparison" in stdout and b"sample.py~L1" in stdout, stdout
                assert (b"synthetic formatter failed" in stdout) == (case == "format-error"), stdout
            elif case == "unchanged":
                assert stdout.decode() == "✅ ecosystem check detected no linter changes.\n"
            else:
                assert b"F401" in stdout and b"fixture/changed" in stdout, stdout
            if case == "checker-error":
                assert b"synthetic checker failed" in stdout and b"fixture/failed" in stdout, stdout
            if case == "git-error":
                assert b"Failed to pull:" in stdout and b"missing.git" in stdout, stdout
            print(f"{case}: {len(stdout):,} matching report bytes")


if __name__ == "__main__":
    main()
