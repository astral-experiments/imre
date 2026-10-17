"""Read counted task and change lines; malformed framing is an input error."""

from .model import Task


def read_problem() -> tuple[list[Task], list[str]]:
    count = int(input())
    assert count >= 0
    tasks: list[Task] = []
    for task_id in range(count):
        words = input().split()
        assert len(words) >= 2
        dependencies: list[str] = []
        for position, word in enumerate(words):
            if position >= 2:
                dependencies.append(word)
        tasks.append(Task(words[0], int(words[1]), dependencies))
    changed_count = int(input())
    assert changed_count >= 0
    changes: list[str] = []
    for change_id in range(changed_count):
        words = input().split()
        assert len(words) == 1
        changes.append(words[0])
    return (tasks, changes)
