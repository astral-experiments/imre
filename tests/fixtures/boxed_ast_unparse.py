import ast
from ast import unparse


def current():
    expression = ast.parse("a * (b + c)", mode="eval")
    assert unparse(expression) == "a * (b + c)"
    expression.body.right.right = ast.Name(id="d", ctx=ast.Load())
    assert ast.unparse(expression) == "a * (b + d)"
    assert unparse(ast.Constant(value="quoted")) == "'quoted'"

    tree = ast.parse("@typing.overload\ndef f(x):\n  return [i for i in x]")
    assert unparse(tree.body[0].decorator_list[0]) == "typing.overload"
    assert unparse(tree) == "@typing.overload\ndef f(x):\n    return [i for i in x]"
