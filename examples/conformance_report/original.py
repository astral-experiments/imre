"""Offline input adapter around the unchanged upstream report implementation.

Input discovery and ty execution happen once in prepare.py. The upstream regex,
record construction, grouping, classification, sorting and renderers run here.
This is an adapter benchmark, not the unchanged upstream command-line program.
"""

import argparse
import importlib.util
import json
import sys
from dataclasses import dataclass
from pathlib import Path


def load_upstream():
    filename = Path(__file__).parent / "upstream" / "conformance.py"
    spec = importlib.util.spec_from_file_location("pinned_conformance", filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass(slots=True)
class PreparedSource:
    path: Path
    text: str

    @property
    def name(self) -> str:
        return self.path.name

    def read_text(self) -> str:
        return self.text


@dataclass(slots=True)
class PreparedInputs:
    revision: str
    paths: dict[str, str]
    sources: list[PreparedSource]
    old: str
    new: str


def read_inputs(manifest_file: str, old_file: str, new_file: str) -> PreparedInputs:
    manifest = json.loads(Path(manifest_file).read_text(encoding="utf-8"))
    revision = manifest["suite_revision"]
    if not isinstance(revision, str):
        raise ValueError("suite revision must be a string")
    paths = {}
    for alias in manifest["diagnostic_paths"]:
        raw, canonical = alias["raw"], alias["path"]
        if not isinstance(raw, str) or not isinstance(canonical, str):
            raise ValueError("diagnostic path aliases must be strings")
        if raw in paths:
            raise ValueError("duplicate diagnostic path alias")
        paths[raw] = canonical
    seen = set()
    sources = []
    for filename in manifest["test_files"]:
        if not isinstance(filename, str):
            raise ValueError("conformance test path must be a string")
        if filename in seen:
            raise ValueError("duplicate conformance test path")
        seen.add(filename)
        path = Path(filename)
        sources.append(PreparedSource(path, path.read_text(encoding="utf-8")))
    return PreparedInputs(revision, paths, sources,
                          Path(old_file).read_text(encoding="utf-8"),
                          Path(new_file).read_text(encoding="utf-8"))


def diagnostics(upstream, text: str, source, paths: dict[str, str]):
    # These are the same field selections and record constructors as upstream's
    # from_gitlab_output. The manifest supplies the already resolved path.
    result = []
    for item in json.loads(text):
        if item["severity"] != "major":
            continue
        position = item["location"]["positions"]
        result.append(upstream.TyDiagnostic(
            check_name=item["check_name"], description=item["description"],
            severity=item["severity"], source=source,
            location=upstream.Location(
                path=Path(paths[item["location"]["path"]]),
                positions=upstream.Positions(
                    begin=upstream.Position(line=position["begin"]["line"], column=position["begin"]["column"]),
                    end=upstream.Position(line=position["end"]["line"], column=position["end"]["column"]),
                ),
            ),
        ))
    return result


def render_report(upstream, inputs: PreparedInputs, format: str, force_summary_table: bool) -> str:
    upstream.CONFORMANCE_DIR_WITH_README = f"https://github.com/python/typing/blob/{inputs.revision}/conformance/"
    upstream.CONFORMANCE_URL = upstream.CONFORMANCE_DIR_WITH_README + "tests/{filename}#L{line}"
    expected = upstream.collect_expected_diagnostics(inputs.sources)
    # Only read_text is supplied by the preloaded source adapter. The upstream
    # records retain ordinary Path objects for grouping, hashing and rendering.
    for error in expected:
        error.location.path = error.location.path.path
    old = diagnostics(upstream, inputs.old, upstream.Source.OLD, inputs.paths)
    new = diagnostics(upstream, inputs.new, upstream.Source.NEW, inputs.paths)
    grouped = upstream.group_diagnostics_by_key(old=old, new=new, expected=expected)
    # collect_file_stats calls resolve() again in upstream. Here the manifest's
    # identities are already resolved for every engine, so retain its algorithm
    # with that redundant filesystem lookup removed from this input adapter.
    path_to_cases = {source.path: [] for source in inputs.sources}
    for case in grouped:
        path_to_cases[case.path].append(case)
    file_stats = [upstream.FileStats(
        path=path,
        old=upstream.compute_stats(cases, upstream.Source.OLD),
        new=upstream.compute_stats(cases, upstream.Source.NEW),
    ) for path, cases in path_to_cases.items()]
    return "\n\n".join(filter(None, [
        upstream.render_summary(grouped, file_stats, force_summary_table=force_summary_table),
        upstream.render_file_stats_table(file_stats),
        upstream.render_test_cases(grouped, format=format),
    ]))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--old", required=True)
    parser.add_argument("--new", required=True)
    parser.add_argument("--format", choices=["github", "diff"], default="github")
    parser.add_argument("--force-summary-table", action="store_true")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--compute", action="store_true")
    parser.add_argument("--rounds", type=int, default=1)
    args = parser.parse_args()
    if args.rounds < 0:
        raise ValueError("compute requires nonnegative rounds")
    inputs = read_inputs(args.manifest, args.old, args.new)
    upstream = load_upstream()
    if args.compute:
        checksum = 0
        rendered = ""
        for _ in range(args.rounds):
            rendered = render_report(upstream, inputs, args.format, args.force_summary_table)
            checksum += len(rendered)
        if args.output is not None:
            args.output.write_text(rendered, encoding="utf-8")
        print(checksum)
    else:
        rendered = render_report(upstream, inputs, args.format, args.force_summary_table)
        if args.output is None:
            print(rendered)
        else:
            args.output.write_text(rendered, encoding="utf-8")


if __name__ == "__main__":
    main()
