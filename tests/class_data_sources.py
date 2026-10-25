"""Build fixed-layout fixtures from byte-for-byte portions of the vendored CPython sources."""

import ast
from pathlib import Path


def write_class_data_sources(project: Path, upstream: Path) -> list[str]:
    def parts(path, classname):
        lines = path.read_text().splitlines(keepends=True)
        module = ast.parse("".join(lines))
        cls = next(n for n in module.body if isinstance(n, ast.ClassDef) and n.name == classname)

        def source(node):
            return "".join(lines[node.lineno - 1:node.end_lineno])

        return module.body, cls.body, source

    def assignments(nodes, names):
        found = {n.targets[0].id: n for n in nodes if isinstance(n, ast.Assign)
                 and len(n.targets) == 1 and isinstance(n.targets[0], ast.Name)}
        return [found[name] for name in names]

    module, body, source = parts(upstream / "textwrap/upstream/Lib/textwrap.py", "TextWrapper")
    whitespace, = assignments(module, ["_whitespace"])
    table, = assignments(body, ["unicode_whitespace_trans"])
    method = next(n for n in body if isinstance(n, ast.FunctionDef) and n.name == "_munge_whitespace")
    wrapper = source(whitespace) + "\nclass TextWrapper:\n" + source(table) + """
    expand_tabs: bool
    tabsize: int
    replace_whitespace: bool

    def __init__(self, expand: bool, size: int, replace: bool):
        self.expand_tabs = expand
        self.tabsize = size
        self.replace_whitespace = replace

""" + source(method) + """
for expand in [False, True]:
    for size in [-1, 0, 3, 8]:
        for replace in [False, True]:
            w = TextWrapper(expand, size, replace)
            for value in ["", "a\\tb\\n", "\\r\\n\\t", "界\\t🙂", "\\x1c\\x1f\\u2028"]:
                print(repr(w._munge_whitespace(value)))
"""
    _, body, source = parts(upstream / "stat/upstream/Lib/test/test_stat.py", "TestFilemode")
    names = ["statmod", "file_flags", "formats", "format_funcs", "stat_struct", "permission_bits", "file_attributes"]
    values = assignments(body, names)
    stat = "class TestFilemode:\n" + "".join(map(source, values)) + "\nclass Child(TestFilemode):\n    pass\n"
    stat += "print(Child.statmod is None)\n"
    for name, node in zip(names[1:], values[1:]):
        original = ast.literal_eval(node.value)
        stat += f"print(len(Child.{name}))\nprint(TestFilemode.{name} is Child().{name})\n"
        for key in sorted(original):
            stat += (f"print(Child.{name}[{key!r}])\n" if isinstance(original, dict)
                     else f"print({key!r} in Child.{name})\n")
    _, body, source = parts(upstream / "shared/upstream/Lib/test/support/__init__.py", "SuppressCrashReport")
    crash = "class SuppressCrashReport:\n" + "".join(map(source, assignments(body, ["old_value", "old_modes"]))) + """
a = SuppressCrashReport()
b = SuppressCrashReport()
a.old_value = 7
v = a.old_value
if v is not None:
    print(v)
print(b.old_value is None)
print(a.old_modes is None)
print(SuppressCrashReport.old_value is None)
a.old_value = None
del a.old_value
print(a.old_value is None)
"""
    programs = {"class_data_textwrap.py": wrapper, "class_data_stat.py": stat, "class_data_test_support.py": crash}
    for name, program in programs.items():
        (project / name).write_text(program)
    return list(programs)
