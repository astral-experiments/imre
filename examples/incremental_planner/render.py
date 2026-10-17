"""Print every rebuilt target and its exact arbitrary-precision timing."""

from .model import Plan


def show(plan: Plan) -> None:
    if plan.error != "":
        print("ERROR")
        print(plan.error)
        print(plan.error_index)
        if plan.error == "negative-duration":
            print(plan.error_value)
        else:
            print(plan.error_name)
        return
    print("OK")
    print("CHANGED")
    print(plan.changed_count)
    print("AFFECTED")
    print(len(plan.order))
    print("PLAN")
    print(len(plan.order))
    for task_id in plan.order:
        start, finish = plan.time_at(task_id)
        print(plan.tasks[task_id].name)
        print(task_id)
        print(plan.tasks[task_id].duration)
        print(start)
        print(finish)
    print("MAKESPAN")
    print(plan.makespan)
