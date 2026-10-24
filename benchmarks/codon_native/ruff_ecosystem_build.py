"""Build the native Codon replay using the pinned Ruff Markdown function bodies."""

import argparse
import ast
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
UPSTREAM = HERE.parents[1] / "examples" / "ruff_ecosystem" / "ruff_ecosystem"


def render_source() -> str:
    check = (UPSTREAM / "check.py").read_text()
    markdown = (UPSTREAM / "markdown.py").read_text()
    definitions = {
        node.name: node for node in ast.parse(check).body if hasattr(node, "name")
    }
    rule_class = definitions["RuleChanges"]
    methods = [node for node in rule_class.body if isinstance(node, ast.FunctionDef)]
    signatures = {
        "update": "self, other",
        "total_added_violations": "self",
        "total_removed_violations": "self",
        "total_added_fixes": "self",
        "total_removed_fixes": "self",
        "total_changes_by_rule": "self",
        "__bool__": "self",
    }

    def with_signature(source, node, args, indent=""):
        body = "\n".join(source.splitlines()[node.body[0].lineno - 1 : node.end_lineno])
        return f"{indent}def {node.name}({args}):\n" + body.replace(
            "Counter()", "Counter[str]()"
        )

    rule_methods = [
        with_signature(check, node, signatures[node.name], "    ")
        for node in methods
        if node.name in signatures
    ]
    markdown_args = {
        "markdown_details": "summary, content, preface",
        "markdown_project_section": "title, content, options, project",
        "markdown_plus_minus": "added, removed",
    }
    rendering = [
        with_signature(markdown, node, markdown_args[node.name])
        for node in ast.parse(markdown).body
        if isinstance(node, ast.FunctionDef)
    ]
    constants = [
        ast.get_source_segment(check, node)
        for node in ast.parse(check).body
        if isinstance(node, ast.Assign)
        and node.targets[0].id
        in {
            "CHECK_SUMMARY_LINE_RE",
            "CHECK_DIFF_LINE_RE",
            "CHECK_DIAGNOSTIC_LINE_RE",
            "GITHUB_MAX_COMMENT_LENGTH",
        }
    ]
    rendering = (
        constants
        + rendering
        + [with_signature(check, definitions["markdown_check_result"], "result")]
    )
    template = (HERE / "ruff_ecosystem.codon").read_text()
    return template.replace(
        "    # UPSTREAM_RULE_METHODS", "\n\n".join(rule_methods)
    ).replace("# UPSTREAM_MARKDOWN", "\n\n".join(rendering))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codon", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    source = args.output.with_suffix(".codon")
    source.write_text(render_source())
    subprocess.run(
        [args.codon, "build", "-release", source, "-o", args.output], check=True
    )


if __name__ == "__main__":
    main()
