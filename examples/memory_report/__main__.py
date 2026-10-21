from sys import argv

from .load import load_report
from .model import ProjectComparison
from .render import render_summary


def project_name(path: str) -> str:
    # Match Path.stem for ordinary file paths, including dotfiles and suffixes.
    start = 0
    for index, character in enumerate(path):
        if character == "/":
            start = index + 1
    name = path[start:]
    suffix = -1
    has_non_dot = False
    for index, character in enumerate(name):
        if character != ".":
            has_non_dot = True
        elif has_non_dot:
            suffix = index
    return name[:suffix] if suffix >= 0 else name


def main() -> None:
    old_path = ""
    new_path = ""
    output_path: str | None = None
    index = 1
    if len(argv) > 1 and argv[1] == "compare":
        index = 2
    while index < len(argv):
        flag = argv[index]
        if index + 1 >= len(argv):
            raise ValueError("expected a value after " + flag)
        value = argv[index + 1]
        if flag == "--old":
            old_path = value
        elif flag == "--new":
            new_path = value
        elif flag == "--output":
            output_path = value
        else:
            raise ValueError("unknown argument: " + flag)
        index += 2
    if not old_path or not new_path:
        raise ValueError("expected --old OLD.json --new NEW.json [--output REPORT.md]")
    old = load_report(old_path)
    new = load_report(new_path)
    rendered = render_summary([ProjectComparison(name=project_name(new_path), old=old, new=new)])
    if output_path is None:
        print(rendered)
    else:
        with open(output_path, "w", encoding="utf-8") as output:
            output.write(rendered)
            output.flush()


main()
