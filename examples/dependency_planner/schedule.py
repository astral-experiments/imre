"""Schedule a dependency graph, choosing the smallest ready task ID first."""

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


def failure(tasks: list[Task], error: str, task_id: int, detail: int) -> Plan:
    return Plan(tasks, [], [], [], [], 0, error, task_id, detail)


def schedule(tasks: list[Task]) -> Plan:
    count = len(tasks)
    successors: list[list[int]] = []
    indegrees = [0] * count
    starts = [0] * count
    finishes = [0] * count
    parents = [-1] * count
    for task_id in range(count):
        outgoing: list[int] = []
        successors.append(outgoing)
    for task_id in range(count):
        task = tasks[task_id]
        if task.duration < 0:
            return failure(tasks, "negative-duration", task_id, task.duration)
        for position in range(len(task.dependencies)):
            dependency = task.dependencies[position]
            if dependency < 0 or dependency >= count:
                return failure(tasks, "missing-dependency", task_id, dependency)
            successors[dependency].append(task_id)
            indegrees[task_id] += 1

    ready: list[int] = []
    order: list[int] = []
    for task_id in range(count):
        if indegrees[task_id] == 0:
            push_ready(ready, task_id)
    for step in range(count):
        if len(ready) == 0:
            break
        task_id = pop_ready(ready)
        order.append(task_id)
        finishes[task_id] = starts[task_id] + tasks[task_id].duration
        outgoing = successors[task_id]
        for position in range(len(outgoing)):
            child = outgoing[position]
            if finishes[task_id] > starts[child]:
                starts[child] = finishes[task_id]
                parents[child] = task_id
            elif finishes[task_id] == starts[child]:
                if parents[child] == -1 or task_id < parents[child]:
                    parents[child] = task_id
            indegrees[child] -= 1
            if indegrees[child] == 0:
                push_ready(ready, child)
    if len(order) != count:
        blocked = -1
        for task_id in range(count):
            if indegrees[task_id] > 0:
                blocked = task_id
                break
        return failure(tasks, "cycle", blocked, -1)

    makespan = 0
    end = -1
    for task_id in range(count):
        if len(successors[task_id]) == 0 and (
            end == -1 or finishes[task_id] > makespan
        ):
            makespan = finishes[task_id]
            end = task_id
    reversed_path: list[int] = []
    while end != -1:
        reversed_path.append(end)
        end = parents[end]
    critical_path: list[int] = []
    for position in range(len(reversed_path) - 1, -1, -1):
        critical_path.append(reversed_path[position])
    return Plan(tasks, order, starts, finishes, critical_path, makespan, "", -1, -1)
