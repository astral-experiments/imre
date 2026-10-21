"""Read whitespace-separated task descriptions without interpreting task names."""

from .model import Task


def read_problem() -> tuple[list[Task], str]:
    count = int(input())
    assert count >= 0
    tasks: list[Task] = []
    for task_id in range(count):
        words = input().split()
        assert len(words) >= 2
        duration = int(words[1])
        dependencies: list[int] = []
        for position in range(2, len(words)):
            dependencies.append(int(words[position]))
        tasks.append(Task(words[0], duration, dependencies))
    lookup_words = input().split()
    assert len(lookup_words) <= 1
    lookup_name = ""
    if len(lookup_words) == 1:
        lookup_name = lookup_words[0]
    return (tasks, lookup_name)
