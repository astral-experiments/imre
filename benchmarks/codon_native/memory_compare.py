"""Native Codon entry point for the memory reporter's file and directory comparisons."""

from pathlib import Path
from sys import argv

from memory_report.model import ProjectComparison, load_report
from memory_report.render import render_summary


def load_directory(directory: str):
    return {path.stem: load_report(path) for path in Path(directory).glob("*.json")}


def main() -> None:
    if len(argv) != 6 or argv[1] != "compare":
        raise ValueError("expected compare with two input paths")
    flags = dict(zip(argv[2::2], argv[3::2]))
    if "--old" in flags and "--new" in flags:
        new = Path(flags["--new"])
        projects = [
            ProjectComparison(
                new.stem, load_report(Path(flags["--old"])), load_report(new)
            )
        ]
    else:
        old = load_directory(flags["--old-dir"])
        new = load_directory(flags["--new-dir"])
        projects = [
            ProjectComparison(name, old[name], new[name])
            for name in sorted(old)
            if name in new
        ]
        if not projects:
            raise ValueError("No common projects found between old and new directories")
    print(render_summary(projects))


main()
