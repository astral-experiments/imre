"""Compile Ruff's formatter Markdown function bodies with native Codon patch types."""

import argparse
import ast
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
UPSTREAM = HERE.parents[1] / "examples/ruff_ecosystem/ruff_ecosystem"


def functions(filename, names):
    source = (UPSTREAM / filename).read_text()
    lines = source.splitlines()
    for node in ast.parse(source).body:
        if isinstance(node, ast.FunctionDef) and node.name in names:
            args = ", ".join(arg.arg for arg in node.args.args)
            body = lines[node.body[0].lineno - 1 : node.end_lineno]
            yield f"def {node.name}({args}):\n" + "\n".join(body)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codon", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    rendering = [
        *functions("markdown.py", {"markdown_project_section", "markdown_details"}),
        *functions("format.py", {"markdown_format_result", "format_patchset"}),
    ]
    template = (HERE / "ruff_ecosystem_format.codon").read_text()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    source = args.output.with_suffix(".codon")
    source.write_text(template.replace("# UPSTREAM_MARKDOWN", "\n\n".join(rendering)))
    subprocess.run(
        [args.codon, "build", "-release", source, "-o", args.output], check=True
    )
