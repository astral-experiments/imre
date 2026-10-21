"""Print the complete schedule, critical path, and a named-task lookup."""

from .model import Plan


def show(plan: Plan, lookup_name: str) -> None:
    if plan.error != "":
        print("ERROR")
        print(plan.error)
        print(plan.error_task)
        print(plan.error_detail)
        return
    print("OK")
    print("PLAN")
    print(len(plan.order))
    for position in range(len(plan.order)):
        task_id = plan.order[position]
        description = plan.tasks[task_id].describe()
        times = plan.time_at(task_id)
        print(description[0])
        print(task_id)
        print(description[-1])
        print(times[0])
        print(times[1])
    print("MAKESPAN")
    print(plan.makespan)
    print("CRITICAL")
    print(len(plan.critical_path))
    path_names = ""
    for position in range(len(plan.critical_path)):
        task_id = plan.critical_path[position]
        print(task_id)
        if position > 0:
            path_names = path_names + " -> "
        path_names = path_names + plan.tasks[task_id].name
    print("PATH")
    print(path_names)
    print("LOOKUP")
    found = plan.find(lookup_name)
    if found is None:
        print("MISSING")
        print(lookup_name)
    else:
        print("FOUND")
        print(found.name)
        print(found.duration)
