import ast

prefix = "".join(" " * level + "if True:\n" for level in range(99))
indent = " " * 99
for parse in (compile, ast.parse):
    parse(prefix + indent + "x = (\n" + indent + "    1)\n", "nested.py", "exec")
    source = prefix + indent + "if True:\n" + indent + " \\\n  pass"
    try:
        parse(source, "nested.py", "exec")
    except IndentationError as error:
        assert error.args == ("too many levels of indentation", ("nested.py", 102, 1, "  pass\n", 102, 0))
        assert str(error) == "too many levels of indentation (nested.py, line 102)"
    else:
        raise AssertionError("accepted a 100th active indent")

for program, expected in ((source + "\nx = 1\n", IndentationError),
                          ("x = 1\n" + source, SyntaxError)):
    try:
        compile(program, "nested.py", "single")
    except SyntaxError as error:
        assert type(error) is expected
    else:
        raise AssertionError("accepted invalid interactive input")
