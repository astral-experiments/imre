"""Exercise Ruff's config patcher against a workspace without running a lint job."""

import sys
from pathlib import Path

import tomli
import tomli_w

from ruff_ecosystem.defaults import DEFAULT_TARGETS
from ruff_ecosystem.projects import ConfigOverrides, rule_name_to_code


def probe(workspace: Path, executable: Path, empty: bool) -> None:
    names = (".ruff.toml", "ruff.toml", "pyproject.toml")
    rule_names = rule_name_to_code(executable)
    # A second lookup must retain the first result, including any new mapping entries.
    rule_names["test-cache-key"] = "T999"
    print(rule_name_to_code(executable)["test-cache-key"] or "<missing>")
    overrides = ConfigOverrides() if empty else ConfigOverrides(
        always={"lint.ignore": ["unused-import", "missing-code", 42], "format.quote-style": "single"},
        when_preview={"lint.extend-select": ["unused-import"]},
        when_no_preview={"lint.per-file-ignores": {"tests/**": ["unused-import", "missing-code"]}},
    )
    for preview in (False, True):
        try:
            with overrides.patch_config(workspace, preview, rule_names):
                for name in names:
                    path = workspace / name
                    if path.exists():
                        print(path.read_text())
                        with path.open("rb") as stream:
                            print(tomli_w.dumps(tomli.load(stream)))
                    else:
                        print("<absent>")
                if not preview:
                    raise ValueError("failed inside patch")
        except ValueError as error:
            print(str(error))
        for name in names:
            path = workspace / name
            print(path.read_text() if path.exists() else "<absent>")


def defaults() -> None:
    print(len(DEFAULT_TARGETS))
    for original in DEFAULT_TARGETS:
        project = original.with_preview_enabled()
        print(project.repo.fullname)
        print(project.check_options.preview)
        print(project.format_options.preview)
        print(project.check_options.select)
        print(original.check_options.preview)
        print(original.format_options.preview)
        print(tomli_w.dumps(project.config_overrides.always))


if len(sys.argv) == 2 and sys.argv[1] == "defaults":
    defaults()
else:
    probe(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3] == "empty")
