"""Write the existing planner report fields to an explicitly owned text stream."""

from typing import TextIO

from incremental_planner.model import Plan


def line(report: TextIO, text: str) -> None:
    report.write(text + "\n")


def show(plan: Plan, report: TextIO) -> None:
    if plan.error != "":
        line(report, "ERROR")
        line(report, plan.error)
        line(report, str(plan.error_index))
        if plan.error == "negative-duration":
            line(report, str(plan.error_value))
        else:
            line(report, plan.error_name)
        return
    line(report, "OK")
    line(report, "CHANGED")
    line(report, str(plan.changed_count))
    line(report, "AFFECTED")
    line(report, str(len(plan.order)))
    line(report, "PLAN")
    line(report, str(len(plan.order)))
    for task_id in plan.order:
        start, finish = plan.time_at(task_id)
        line(report, plan.tasks[task_id].name)
        line(report, str(task_id))
        line(report, str(plan.tasks[task_id].duration))
        line(report, str(start))
        line(report, str(finish))
    line(report, "MAKESPAN")
    line(report, str(plan.makespan))
