"""Validate the whole graph, then schedule the transitively affected targets."""

from .model import Plan, Task


def push_ready(ready: list[int], task_id: int) -> None:
    ready.append(task_id)
    position = len(ready) - 1
    while position > 0:
        parent = (position - 1) // 2
        if ready[parent] <= task_id:
            break
        ready[position] = ready[parent]
        position = parent
    ready[position] = task_id


def pop_ready(ready: list[int]) -> int:
    result = ready[0]
    last = ready.pop()
    count = len(ready)
    if count > 0:
        position = 0
        while position * 2 + 1 < count:
            child = position * 2 + 1
            if child + 1 < count and ready[child + 1] < ready[child]:
                child += 1
            if ready[child] >= last:
                break
            ready[position] = ready[child]
            position = child
        ready[position] = last
    return result


def failure(tasks: list[Task], error: str, index: int, name: str, value: int) -> Plan:
    return Plan(tasks, [], [], [], 0, 0, error, index, name, value)


def schedule(tasks: list[Task], changes: list[str]) -> Plan:
    count = len(tasks)
    names: dict[str, int] = {}
    for task_id, task in enumerate(tasks):
        if task.name in names:
            return failure(tasks, "duplicate-name", task_id, task.name, 0)
        names[task.name] = task_id

    # Dict insertion order is task input order, including for error precedence.
    for name, task_id in names.items():
        if tasks[task_id].duration < 0:
            return failure(
                tasks, "negative-duration", task_id, name, tasks[task_id].duration
            )

    dependencies: list[list[int]] = []
    successors: list[list[int]] = []
    for task in tasks:
        dependencies.append([])
        successors.append([])
    indegrees = [0] * count
    for task_id, task in enumerate(tasks):
        seen: set[str] = set()
        for name in task.dependencies:
            if name not in seen:
                found_dependency = names.get(name)
                if found_dependency is None:
                    return failure(tasks, "unknown-dependency", task_id, name, 0)
                seen.add(name)
                dependencies[task_id].append(found_dependency)
                successors[found_dependency].append(task_id)
                indegrees[task_id] += 1

    changed: set[int] = set()
    for change_id, name in enumerate(changes):
        changed_id = names.get(name)
        if changed_id is None:
            return failure(tasks, "unknown-change", change_id, name, 0)
        changed.add(changed_id)

    ready: list[int] = []
    for task_id, degree in enumerate(indegrees):
        if degree == 0:
            push_ready(ready, task_id)
    completed = 0
    while len(ready) > 0:
        task_id = pop_ready(ready)
        completed += 1
        for child in successors[task_id]:
            indegrees[child] -= 1
            if indegrees[child] == 0:
                push_ready(ready, child)
    if completed != count:
        for task_id, degree in enumerate(indegrees):
            if degree > 0:
                return failure(tasks, "cycle", task_id, tasks[task_id].name, 0)

    # Hash iteration order may vary. Only the closure is used by the schedule.
    affected: set[int] = set()
    queue: list[int] = []
    for task_id in changed:
        affected.add(task_id)
        queue.append(task_id)
    cursor = 0
    while cursor < len(queue):
        task_id = queue[cursor]
        for child in successors[task_id]:
            if child not in affected:
                affected.add(child)
                queue.append(child)
        cursor += 1

    starts = [0] * count
    finishes = [0] * count
    remaining = [0] * count
    for task_id, task in enumerate(tasks):
        if task_id in affected:
            for dependency in dependencies[task_id]:
                if dependency in affected:
                    remaining[task_id] += 1
            if remaining[task_id] == 0:
                push_ready(ready, task_id)
    order: list[int] = []
    makespan = 0
    while len(ready) > 0:
        task_id = pop_ready(ready)
        order.append(task_id)
        finishes[task_id] = starts[task_id] + tasks[task_id].duration
        if finishes[task_id] > makespan:  # noqa: PLR1730 - max is outside the compiled subset.
            makespan = finishes[task_id]
        for child in successors[task_id]:
            if finishes[task_id] > starts[child]:  # noqa: PLR1730 - max is outside the compiled subset.
                starts[child] = finishes[task_id]
            remaining[child] -= 1
            if remaining[child] == 0:
                push_ready(ready, child)
    assert len(order) == len(affected)
    return Plan(tasks, order, starts, finishes, len(changed), makespan, "", -1, "", 0)
