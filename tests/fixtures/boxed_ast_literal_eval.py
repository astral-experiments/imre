import ast
from ast import literal_eval


def current():
    source = "1_0.5"
    value = literal_eval(source.replace("_", ""))
    assert type(value) is float and value == 10.5
    assert repr(literal_eval("-0.0")) == "-0.0"
    assert literal_eval("(-2.5-3j, b'x', ..., {1, 1}, set())") == (-2.5-3j, b"x", ..., {1}, set())
    assert literal_eval("{'x': 1, 'x': [True, None]}") == {"x": [True, None]}
    tree = ast.parse("1+0j", mode="eval")
    tree.body.right = ast.Constant(6j)
    assert literal_eval(node_or_string=tree) == 1+6j
    assert tree.body.op is ast.parse("1+2", mode="eval").body.op
    for text in ("foo()", "set([1])", "a[1]", "3+4", "3+-2j", "++1", "+True",
                 "{**{}}", "[x for x in (1,)]", "(lambda: 42)()",
                 "__import__('builtins').print('untrusted expression executed')"):
        try:
            literal_eval(text)
        except ValueError:
            pass
        else:
            raise AssertionError(text)
    try:
        literal_eval("[1,\n ++6]")
    except ValueError as error:
        assert "malformed node or string on line 2:" in str(error)
    else:
        raise AssertionError("accepted a nested unary expression")
    try:
        literal_eval("\n 2")
    except IndentationError as error:
        assert isinstance(error, SyntaxError)
    else:
        raise AssertionError("ignored indentation after the first line")
    for text in ("1;", "0x", "\0", "(" * 201 + "1" + ")" * 201):
        try:
            literal_eval(text)
        except SyntaxError:
            pass
        else:
            raise AssertionError("accepted invalid syntax")
    for text in ("{[]}", "{[]: 1}"):
        try:
            literal_eval(text)
        except TypeError:
            pass
        else:
            raise AssertionError("accepted an unhashable element")
    cycle = ast.List(elts=[])
    cycle.elts.append(cycle)
    try:
        literal_eval(cycle)
    except RecursionError:
        pass
    else:
        raise AssertionError("accepted a cyclic node")
    print("native literal values and rejection controls")
