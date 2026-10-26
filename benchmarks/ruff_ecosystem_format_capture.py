"""Freeze the actual diffs and errors returned by Ruff's ruff-and-ruff command."""

import argparse
import hashlib
import json
import sys
from pathlib import Path

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "ruff_ecosystem"


def freeze_result(result, revisions, directory):
    sys.path.insert(0, str(EXAMPLE))
    from ruff_ecosystem.defaults import DEFAULT_TARGETS

    directory.mkdir(parents=True, exist_ok=True)
    outcomes = {}
    for kind, group in (("ok", "completed"), ("error", "errored")):
        for project, value in result[group]:
            repo = project["repo"]
            key = f"{repo['owner']}:{repo['name']}"
            if key in outcomes:
                raise ValueError(f"duplicate result for {key}")
            if kind == "ok" and value["repo"]["commit_hash"] != revisions[key]:
                raise ValueError(f"revision mismatch for {key}")
            outcomes[key] = kind, project, value

    rows, files = [], {}
    for index, target in enumerate(DEFAULT_TARGETS):
        owner, name = target.repo.owner, target.repo.name
        key = f"{owner}:{name}"
        kind, project, value = outcomes.pop(key)
        if kind == "ok":
            # ClonedRepository.diff() returns git stdout.decode().splitlines().
            # Preserve empty final lines so replay's splitlines() recovers the
            # exact sequence the original renderer received.
            text = "".join(line + "\n" for line in value["diff"])
        else:
            text = value
        filename = f"{index:02d}.{'patch' if kind == 'ok' else 'error'}"
        data = text.encode("utf-8")
        (directory / filename).write_bytes(data)
        options = project["format_options"]
        fields = [
            owner,
            name,
            revisions[key],
            str(int(options["preview"])),
            options["exclude"],
            kind,
            filename,
        ]
        if any("\t" in field or "\n" in field for field in fields):
            raise ValueError(f"project metadata cannot be written as TSV: {key}")
        rows.append("\t".join(fields))
        files[key] = {
            "revision": revisions[key],
            "kind": kind,
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    if outcomes or set(files) != set(revisions):
        raise ValueError("the captured projects differ from Ruff's defaults")
    (directory / "projects.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    (directory / "capture.json").write_text(
        json.dumps(files, indent=2) + "\n", encoding="utf-8"
    )


def record(directory, revisions, arguments):
    """Observe the actual Result passed to the unchanged Markdown renderer."""
    sys.path.insert(0, str(EXAMPLE))
    from ruff_ecosystem.cli import entrypoint
    from ruff_ecosystem.format import markdown_format_result
    from ruff_ecosystem.main import JSONEncoder

    # Ruff can report either of two configuration errors from a parallel file
    # walk. Capture this invocation's inputs, rather than running Ruff again.
    def before_render(code, offset):
        result = sys._getframe(1).f_locals["result"]
        freeze_result(
            json.loads(json.dumps(result, cls=JSONEncoder)), revisions, directory
        )
        return sys.monitoring.DISABLE

    monitor = sys.monitoring
    tool = monitor.PROFILER_ID
    monitor.use_tool_id(tool, "Ruff format capture")
    monitor.register_callback(tool, monitor.events.PY_START, before_render)
    monitor.set_local_events(
        tool, markdown_format_result.__code__, monitor.events.PY_START
    )
    sys.argv = ["ruff-ecosystem", *arguments]
    entrypoint()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("revisions", type=Path)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    record(args.directory, json.loads(args.revisions.read_bytes()), args.arguments)
