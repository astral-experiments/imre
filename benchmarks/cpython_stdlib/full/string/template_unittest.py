"""Exercise CPython's original Template methods with native TestCase callbacks."""
import ast
import subprocess


# Each control repairs one argument that the original method expects to fail.
# The unchanged method and the separately broken copy both execute in the same native binary.
REPAIRED_INPUT = {
    "test_regular_templates": (b"dict(who='tim'))", b"dict(who='tim', what='ham'))", "KeyError"),
    "test_regular_templates_with_braces": (b"dict(who='tim', what='ham'))", b"d)", "KeyError"),
    "test_invalid_placeholders": (b"Template('$who likes $')", b"Template('$who likes $$')", "ValueError"),
    "test_keyword_arguments_safe": (b"s.substitute, d, {}", b"s.substitute, d", "TypeError"),
}


def straight_line_assertions(method):
    """Count unconditional assertion calls; reject control flow rather than guessing."""
    cls = ast.parse(b"class Case:\n" + method).body[0]
    aliases = {}
    checks = 0
    for stmt in cls.body[0].body:
        if isinstance(stmt, ast.Assign):
            value = stmt.value
            if isinstance(value, ast.Attribute) and isinstance(value.value, ast.Name) and value.value.id == "self":
                assert len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name)
                aliases[stmt.targets[0].id] = value.attr
            continue
        assert isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call)
        callee = stmt.value.func
        if isinstance(callee, ast.Attribute):
            assert isinstance(callee.value, ast.Name) and callee.value.id == "self"
            name = callee.attr
        else:
            name = aliases[callee.id]
        assert name in ("assertRaises", "assertEqual")
        checks += 1
    return checks


def run_method(args, level, work, name, method, compile_run):
    original, repaired, category = REPAIRED_INPUT[name]
    assert method.count(original) == 1
    control = method.replace(original, repaired)
    entry = f"""
TestTemplate().{name}()
print("original returned")
try:
    BrokenTemplate().{name}()
except AssertionError as exc:
    print(str(exc))
""".encode()
    source = (b"import unittest\nfrom string import Template\n\nclass TestTemplate(unittest.TestCase):\n" + method
              + b"\nclass BrokenTemplate(unittest.TestCase):\n" + control + entry)
    reference = subprocess.run([args.python, "-E", "-"], input=source, cwd=work,
                               capture_output=True, check=True)
    assert not reference.stderr
    expected = reference.stdout
    failure = f"{category} not raised by substitute"
    assert expected.splitlines() == [b"original returned", failure.encode()], (name, expected)
    got = compile_run(args, level, source, work)
    assert got == expected, (name, got, expected)
    return straight_line_assertions(method), failure
