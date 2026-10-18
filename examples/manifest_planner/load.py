"""Close each payload before returning its parsed graph or propagating an error."""

from batch_planner.parse import read_problem
from incremental_planner.model import Task


def load_problem(path: str) -> tuple[list[Task], list[str]]:
    try:
        source = open(path, "r", encoding="utf-8")  # noqa: SIM115 -- entered below
    except ValueError:
        raise ValueError("invalid payload path")
    with source:
        return read_problem(source.read().splitlines())
