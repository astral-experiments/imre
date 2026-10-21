from pathlib import Path
from sys import argv

from .load import load_directory, load_report
from .model import ProjectComparison
from .render import render_summary
from .run import run_prepared


def main() -> None:
    old_path = ""
    new_path = ""
    old_directory = ""
    new_directory = ""
    output_path: str | None = None
    projects_path = ""
    old_ty = ""
    new_ty = ""
    run_mode = len(argv) > 1 and argv[1] == "run"
    index = 1
    if len(argv) > 1 and (argv[1] == "compare" or run_mode):
        index = 2
    while index < len(argv):
        flag = argv[index]
        if index + 1 >= len(argv):
            raise ValueError("expected a value after " + flag)
        value = argv[index + 1]
        if run_mode and flag == "--projects":
            projects_path = value
        elif run_mode and flag == "--old-ty":
            old_ty = value
        elif run_mode and flag == "--new-ty":
            new_ty = value
        elif not run_mode and flag == "--old":
            old_path = value
        elif not run_mode and flag == "--new":
            new_path = value
        elif not run_mode and flag == "--old-dir":
            old_directory = value
        elif not run_mode and flag == "--new-dir":
            new_directory = value
        elif flag == "--output":
            output_path = value
        else:
            raise ValueError("unknown argument: " + flag)
        index += 2
    comparisons: list[ProjectComparison] = []
    if run_mode:
        if not projects_path or not old_ty or not new_ty:
            raise ValueError("expected run --projects MANIFEST --old-ty ABS --new-ty ABS [--output REPORT.md]")
        comparisons = run_prepared(Path(projects_path), old_ty, new_ty)
    elif old_path and new_path:
        new_file = Path(new_path)
        comparisons.append(ProjectComparison(
            name=new_file.stem, old=load_report(Path(old_path)), new=load_report(new_file),
        ))
    elif old_directory and new_directory:
        old_reports = load_directory(Path(old_directory))
        new_reports = load_directory(Path(new_directory))
        common = sorted(name for name in old_reports if name in new_reports)
        if not common:
            raise ValueError("No common projects found between old and new directories")
        for name in common:
            comparisons.append(ProjectComparison(name=name, old=old_reports[name], new=new_reports[name]))
    else:
        raise ValueError("expected --old/--new or --old-dir/--new-dir [--output REPORT.md]")
    rendered = render_summary(comparisons)
    if output_path is None:
        print(rendered)
    else:
        Path(output_path).write_text(rendered, encoding="utf-8")


main()
