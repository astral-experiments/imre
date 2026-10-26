import ast


def current():
    tree = ast.parse("if ready:\n    value = source.method('X')\n")
    for node in ast.walk(tree):
        if isinstance(node, ast.Name):
            node.id = "." + node.id
    condition = tree.body[0]
    assign = condition.body[0]
    print(condition.test.id, assign.targets[0].id, assign.value.func.value.id)
    print(assign.value.func.attr, assign.value.args[0].value, assign.lineno)

    # Children of a queued node are read when traversal reaches that node.
    for node in ast.walk(tree):
        if node is tree:
            assign.value = ast.Name("new", ast.Load())
        if isinstance(node, ast.Name):
            print(node.id)
    expression = ast.parse("é", mode="eval").body
    print(isinstance(expression, ast.expr), expression.end_col_offset)
    print(expression.ctx is ast.parse("x", mode="eval").body.ctx)
    walk = ast.walk(ast.parse("x", mode="eval"))
    for node in walk:
        print(type(node).__name__)
        break
    print([type(node).__name__ for node in walk], list(walk))
    first = ast.Module(body=[], type_ignores=[])
    second = ast.Module(body=[], type_ignores=[])
    first.body.append(ast.Return(ast.Constant("one")))
    print(len(first.body), len(second.body), first.body[0].value.value)
