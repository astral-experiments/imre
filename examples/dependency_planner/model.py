"""Fixed-layout task and plan records shared by the package modules."""


class Task:
    name: str
    duration: int
    dependencies: list[int]

    def __init__(self, name: str, duration: int, dependencies: list[int]) -> None:
        self.name = name
        self.duration = duration
        self.dependencies = dependencies

    def describe(self) -> tuple[str, int]:
        return (self.name, self.duration)


class Plan:
    tasks: list[Task]
    order: list[int]
    starts: list[int]
    finishes: list[int]
    critical_path: list[int]
    makespan: int
    error: str
    error_task: int
    error_detail: int

    def __init__(
        self,
        tasks: list[Task],
        order: list[int],
        starts: list[int],
        finishes: list[int],
        critical_path: list[int],
        makespan: int,
        error: str,
        error_task: int,
        error_detail: int,
    ) -> None:
        self.tasks = tasks
        self.order = order
        self.starts = starts
        self.finishes = finishes
        self.critical_path = critical_path
        self.makespan = makespan
        self.error = error
        self.error_task = error_task
        self.error_detail = error_detail

    def find(self, name: str) -> Task | None:
        for index in range(len(self.tasks)):
            task = self.tasks[index]
            if task.name == name:
                return task
        return None

    def time_at(self, task_id: int) -> tuple[int, int]:
        return (self.starts[task_id], self.finishes[task_id])
