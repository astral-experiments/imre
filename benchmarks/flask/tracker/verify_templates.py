"""Compare generated row loops with Jinja and reject unsupported constructs."""

import runpy
import tempfile
from pathlib import Path

from jinja2 import Environment
from jinja2.frozen_rows import write_renderer

from presentation import LIST


def main():
    env = Environment(autoescape=True)
    rows = [
        {"id": "7", "title": "<'résumé'> & 東京", "status": "open"},
        {"id": "8", "title": '"Über"', "status": "closed"},
    ]
    with tempfile.TemporaryDirectory() as tmp:
        target = Path(tmp) / "renderer.py"
        for variable, source in [
            ("issues", LIST),
            ("tasks", "α{% for t in tasks %}{{ t.status }}|{{ t.title }}|{{ t.status }}{% endfor %}ω"),
        ]:
            write_renderer(env, source, target)
            render = runpy.run_path(str(target))["render"]
            for items in ([], rows):
                assert render(items) == env.from_string(source).render(**{variable: items})
        for fragment in (
            "{{ task.title | safe }}",
            "{{ task.items }}",  # An attribute of a dict can shadow a mapping key.
            "{% for part in task.tags %}{{ part }}{% endfor %}",
        ):
            try:
                write_renderer(env, "{% for task in tasks %}" + fragment + "{% endfor %}", target)
            except ValueError:
                continue
            raise AssertionError(f"unexpectedly accepted row body {fragment}")


if __name__ == "__main__":
    main()
