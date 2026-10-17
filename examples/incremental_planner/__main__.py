"""Read a graph and changed targets, then print a deterministic rebuild plan."""

from .parse import read_problem
from .render import show
from .schedule import schedule


def main() -> None:
    tasks, changes = read_problem()
    show(schedule(tasks, changes))


main()
