import ast


class DroppingChild(ast.AST):
    _fields = ("first", "second")

    @property
    def second(self):
        self.first = None
        for i in range(9000):
            unused = [{"n": i}, (i, i)]
        return None


class Reentrant(ast.AST):
    _fields = ("child",)

    @property
    def child(self):
        try:
            for unused in self.cursor:
                break
            raise AssertionError("running walk accepted a second caller")
        except ValueError as error:
            assert str(error) == "generator already executing"
        return self.payload


class Broken(ast.AST):
    _fields = ("bad",)

    @property
    def bad(self):
        raise ValueError("bad child")


class GrowingFields(ast.AST):
    @property
    def first(self):
        self._fields.append("tail")
        return None


def current():
    decorated_source = (
        "if ready:\n"
        "    @first\n"
        "    @second(\n"
        "        value\n"
        "    )\n"
        "    # def in a comment\n"
        "    def \\\n"
        "      f(): pass\n"
    )
    decorated = ast.parse(decorated_source).body[0].body[0]
    print(decorated.lineno, decorated.col_offset, decorated.end_lineno)
    print([item.lineno for item in decorated.decorator_list])
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
    # Postfix operators do not need nesting delimiters.
    parts = ["x"]
    for _ in range(6000):
        parts.append(".field()[0]")
    deep = ast.parse("".join(parts), mode="eval").body
    print(type(deep).__name__, type(deep.value.func).__name__, deep.end_col_offset)
    expression = ast.parse("é", mode="eval").body
    print(isinstance(expression, ast.expr), expression.end_col_offset)

    for newline in ("\r", "\n", "\r\n"):
        tree = ast.parse("first = 1" + newline + "second = 2")
        second = tree.body[1]
        assert (second.lineno, second.col_offset, second.end_lineno, second.end_col_offset) == (
            2, 0, 2, 10
        )

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

    root = DroppingChild()
    root.first = ast.Name("retained", ast.Load())
    assert [node.id for node in ast.walk(root) if isinstance(node, ast.Name)] == ["retained"]

    root = Reentrant()
    root.payload = ast.Name("child", ast.Load())
    root.cursor = ast.walk(root)
    assert [type(n).__name__ for n in root.cursor] == ["Reentrant", "Name", "Load"]

    cursor = ast.walk(ast.Module(body=[Broken(), ast.Constant("following")], type_ignores=[]))
    try:
        list(cursor)
    except ValueError as error:
        assert str(error) == "bad child"
    else:
        raise AssertionError("missing child lookup error")
    assert list(cursor) == []

    root = GrowingFields()
    root._fields = ["first"]
    root.tail = ast.Name("added", ast.Load())
    assert [node.id for node in ast.walk(root) if isinstance(node, ast.Name)] == ["added"]
