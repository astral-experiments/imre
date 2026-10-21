"""Plan each manifest entry independently and recover only input-side failures."""

import sys
from typing import TextIO

from incremental_planner.model import Task
from incremental_planner.schedule import schedule

from .load import load_problem
from .render import line, show


def process(path: str, request_id: int, report: TextIO) -> bool:
    line(report, "REQUEST")
    line(report, str(request_id))
    # Make sequential opens deterministic even when the payload aliases report.
    report.flush()
    tasks: list[Task] = []
    changes: list[str] = []
    kind = ""
    message = ""
    try:
        tasks, changes = load_problem(path)
    except UnicodeDecodeError:
        kind = "UnicodeDecodeError"
        message = "invalid UTF-8"
    except OSError:
        kind = "OSError"
        message = "cannot read payload"
    except ValueError as error:
        kind = "ValueError"
        message = str(error)
    except IndexError:
        kind = "IndexError"
        message = "truncated payload"
    except AssertionError as error:
        kind = "AssertionError"
        message = str(error)
    if kind == "":
        show(schedule(tasks, changes), report)
    else:
        line(report, "INVALID")
        line(report, kind)
        line(report, message)
    line(report, "END")
    line(report, str(request_id))
    return kind == ""


def main() -> None:
    if len(sys.argv) != 3:
        raise ValueError("usage: manifest_planner MANIFEST REPORT")
    with open(sys.argv[1], "r", encoding="utf-8") as manifest:
        paths = manifest.read().splitlines()
    assert manifest.closed
    completed = 0
    invalid = 0
    cleaned = 0
    with open(sys.argv[2], "w", encoding="utf-8") as report:
        for request_id, path in enumerate(paths):
            if process(path, request_id, report):
                completed += 1
            else:
                invalid += 1
            cleaned += 1
        line(report, "SUMMARY")
        line(report, "COMPLETED")
        line(report, str(completed))
        line(report, "INVALID")
        line(report, str(invalid))
        line(report, "CLEANED")
        line(report, str(cleaned))
        report.flush()
    assert report.closed


main()
