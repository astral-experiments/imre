"""Parse one already collected frame without reading from the next request."""

from incremental_planner.model import Task


def integer(text: str) -> int:
    try:
        return int(text)
    except ValueError:
        raise ValueError("invalid integer")


def read_problem(lines: list[str]) -> tuple[list[Task], list[str]]:
    count = integer(lines[0])
    if count < 0:
        raise ValueError("negative target count")
    position = 1
    tasks: list[Task] = []
    for task_id in range(count):
        words = lines[position].split()
        position += 1
        assert len(words) >= 2, "target row needs a name and duration"
        dependencies: list[str] = []
        for word_id, word in enumerate(words):
            if word_id >= 2:
                dependencies.append(word)
        tasks.append(Task(words[0], integer(words[1]), dependencies))
    changed_count = integer(lines[position])
    position += 1
    if changed_count < 0:
        raise ValueError("negative change count")
    changes: list[str] = []
    for change_id in range(changed_count):
        words = lines[position].split()
        position += 1
        assert len(words) == 1, "change row needs exactly one name"
        changes.append(words[0])
    return (tasks, changes)
