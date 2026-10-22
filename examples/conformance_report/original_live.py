"""Live input/publication adapter around the unchanged pinned upstream report.

The upstream discovery, analyzer argv, JSON conversion, grouping, statistics,
and rendering functions run unchanged. This adapter supplies the bounded CLI,
child cwd/timeout, schema checks, and atomic publication contract.
"""

import argparse
import contextlib
import math
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace

from original import load_upstream


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", required=True, type=Path)
    parser.add_argument("--old-ty", required=True, type=Path)
    parser.add_argument("--new-ty", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--suite-revision", default="835b70f3f795f8e061b1f7621178515b0a77a0ad")
    parser.add_argument("--format", default="github", choices=("github", "diff"))
    parser.add_argument("--force-summary-table", action="store_true")
    parser.add_argument("--timeout", type=float, default=15.0)
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        raise ValueError("timeout must be finite and positive")
    suite = args.suite.resolve(strict=True)
    old_ty = args.old_ty.resolve(strict=True)
    new_ty = args.new_ty.resolve(strict=True)
    output = args.output.absolute()
    upstream = load_upstream()
    upstream.CONFORMANCE_DIR_WITH_README = f"https://github.com/python/typing/blob/{args.suite_revision}/conformance/"
    upstream.CONFORMANCE_URL = upstream.CONFORMANCE_DIR_WITH_README + "tests/{filename}#L{line}"

    def run(command, **kwargs):
        kwargs.update(cwd=suite, env=os.environ.copy(), encoding="utf-8", timeout=args.timeout)
        return subprocess.run(command, **kwargs)

    upstream.subprocess = SimpleNamespace(run=run)
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".relar-conformance-") as temporary:
        # Schema validation is an adapter boundary: upstream consumes table keys.
        groups = upstream.tomllib.loads((suite / "src" / "test_groups.toml").read_text(encoding="utf-8"))
        for group in groups.values():
            if not isinstance(group, dict) or not isinstance(group.get("name"), str) or not isinstance(group.get("href"), str):
                raise ValueError("test groups require string name and href fields")
        files = upstream.get_test_cases(upstream.get_test_groups(suite), suite / "tests")
        if not files:
            raise ValueError("no conformance test files discovered")
        with contextlib.chdir(suite):
            expected = upstream.collect_expected_diagnostics(files)
            old = upstream.collect_ty_diagnostics([str(old_ty)], upstream.Source.OLD, files, extra_search_paths=[suite / "tests"])
            new = upstream.collect_ty_diagnostics([str(new_ty)], upstream.Source.NEW, files, extra_search_paths=[suite / "tests"])
            grouped = upstream.group_diagnostics_by_key(old=old, new=new, expected=expected)
            stats = upstream.collect_file_stats(grouped, files)
            rendered = "\n\n".join(filter(None, [
                upstream.render_summary(grouped, stats, force_summary_table=args.force_summary_table),
                upstream.render_file_stats_table(stats),
                upstream.render_test_cases(grouped, format=args.format),
            ]))
        staged = Path(temporary) / "report.md"
        staged.write_text(rendered, encoding="utf-8")
        staged.replace(output)


if __name__ == "__main__":
    main()
