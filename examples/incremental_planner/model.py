"""Fixed-layout records; dependency names are resolved after parsing."""


class Task:
    name: str
    duration: int
    dependencies: list[str]

    def __init__(self, name: str, duration: int, dependencies: list[str]) -> None:
        self.name = name
        self.duration = duration
        self.dependencies = dependencies


class Plan:
    tasks: list[Task]
    order: list[int]
    starts: list[int]
    finishes: list[int]
    changed_count: int
    makespan: int
    error: str
    error_index: int
    error_name: str
    error_value: int

    def __init__(
        self,
        tasks: list[Task],
        order: list[int],
        starts: list[int],
        finishes: list[int],
        changed_count: int,
        makespan: int,
        error: str,
        error_index: int,
        error_name: str,
        error_value: int,
    ) -> None:
        self.tasks = tasks
        self.order = order
        self.starts = starts
        self.finishes = finishes
        self.changed_count = changed_count
        self.makespan = makespan
        self.error = error
        self.error_index = error_index
        self.error_name = error_name
        self.error_value = error_value

    def time_at(self, task_id: int) -> tuple[int, int]:
        return (self.starts[task_id], self.finishes[task_id])
