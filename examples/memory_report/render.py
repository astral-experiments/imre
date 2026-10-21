from .compare import diff_items
from .model import ProjectComparison


def format_bytes(byte_count: int) -> str:
    value = float(byte_count)
    for unit in ["B", "kB", "MB"]:
        magnitude = -value if value < 0.0 else value
        if magnitude < 1024.0:
            return f"{value:.2f}{unit}"
        value /= 1024.0
    return f"{value:.2f}GB"


def format_diff(old_bytes: int, new_bytes: int) -> str:
    difference = new_bytes - old_bytes
    if difference == 0:
        return "-"
    sign = "+" if difference > 0 else ""
    if old_bytes == 0:
        return f"{sign}{format_bytes(difference)} (new)"
    magnitude = -difference if difference < 0 else difference
    return f"{sign}{difference / old_bytes:.2%} ({format_bytes(magnitude)})"


def format_outcome(old_bytes: int, new_bytes: int) -> str:
    if new_bytes > old_bytes:
        return "⏫"
    if new_bytes < old_bytes:
        return "⬇️"
    return "✅"


def render_summary(projects: list[ProjectComparison]) -> str:
    if not projects:
        return "No memory reports to compare."
    projects = sorted(projects, key=lambda project: project.total_diff_bytes, reverse=True)
    any_increased = any(project.total_diff_bytes > 10_000 for project in projects)
    any_decreased = any(project.total_diff_bytes < -10_000 for project in projects)
    lines = ["## Memory usage report", ""]
    if not (any_increased or any_decreased):
        lines.append("Memory usage unchanged ✅")
        return "\n".join(lines)
    lines.extend([
        "### Summary", "",
        "| Project | Old | New | Diff | Outcome |",
        "|---------|-----|-----|------|---------|",
    ])
    for project in projects:
        old_bytes = project.old.total_bytes
        new_bytes = project.new.total_bytes
        lines.append(
            f"| {project.name} | {format_bytes(old_bytes)} | {format_bytes(new_bytes)} | "
            f"{format_diff(old_bytes, new_bytes)} | {format_outcome(old_bytes, new_bytes)} |"
        )
    lines.extend([
        "", "### Significant changes", "", "<details>",
        "<summary>Click to expand detailed breakdown</summary>", "",
    ])
    for project in projects:
        differences = diff_items(
            [*project.old.structs, *project.old.queries],
            [*project.new.structs, *project.new.queries],
        )
        if not differences:
            continue
        lines.extend([
            f"### {project.name}", "",
            "| Name | Old | New | Diff | Outcome |",
            "|------|-----|-----|------|---------|",
        ])
        for item in differences[:15]:
            lines.append(
                f"| `{item.name}` | {format_bytes(item.old_bytes)} | {format_bytes(item.new_bytes)} | "
                f"{format_diff(item.old_bytes, item.new_bytes)} |"
                f"{format_outcome(item.old_bytes, item.new_bytes)} |"
            )
        remaining = len(differences) - 15
        if remaining > 0:
            lines.append(f"| ... | | | *{remaining} more* |")
        lines.append("")
    lines.extend(["</details>", ""])
    return "\n".join(lines)
