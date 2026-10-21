"""Read, schedule, and report a dependency graph from standard input."""

import dependency_planner.render as render  # noqa: PLR0402 - exercises a module alias

from .parse import read_problem
from .schedule import schedule


def main() -> None:
    problem = read_problem()
    plan = schedule(problem[0])
    render.show(plan, problem[1])


main()
