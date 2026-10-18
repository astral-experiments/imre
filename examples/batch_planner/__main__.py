"""Recover from malformed payloads while preserving later request boundaries."""

from incremental_planner.render import show
from incremental_planner.schedule import schedule

from .parse import read_problem


def process(lines: list[str], request_id: int) -> bool:
    print("REQUEST")
    print(request_id)
    completed = False
    try:
        tasks, changes = read_problem(lines)
    except ValueError as error:
        print("INVALID")
        print("ValueError")
        print(str(error))
    except IndexError:
        print("INVALID")
        print("IndexError")
        print("truncated payload")
    except AssertionError as error:
        print("INVALID")
        print("AssertionError")
        print(str(error))
    else:
        show(schedule(tasks, changes))
        completed = True
    finally:
        for cleanup_id in range(len(lines)):
            lines.pop()
        print("END")
        print(request_id)
    return completed


def main() -> None:
    request_count = int(input())
    assert request_count >= 0
    completed = 0
    invalid = 0
    cleaned = 0
    for request_id in range(request_count):
        line_count = int(input())
        assert line_count >= 0
        payload: list[str] = []
        for line_id in range(line_count):
            payload.append(input())
        if process(payload, request_id):
            completed += 1
        else:
            invalid += 1
        assert len(payload) == 0
        cleaned += 1
    print("SUMMARY")
    print("COMPLETED")
    print(completed)
    print("INVALID")
    print(invalid)
    print("CLEANED")
    print(cleaned)


main()
