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


class ReplacingFields(ast.AST):
    @property
    def first(self):
        self._fields = ()
        for i in range(9000):
            unused = [{"n": i}, (i, i)]
        return None


class CustomFieldOrder(tuple):
    def __iter__(self):
        yield "tail"


def check_import_nodes():
    import sys
    from ast import Import, ImportFrom, alias

    assert (Import, ImportFrom, alias) == (ast.Import, ast.ImportFrom, ast.alias)

    plain, relative_future, relative, star = ast.parse(
        "import _imre_ast_fixture_missing.é as local, π\n"
        "from .__future__ import example\n"
        "from .. import (\n    first as other,\n    last,\n)\n"
        "from pkg import *"
    ).body
    assert "_imre_ast_fixture_missing" not in sys.modules
    assert type(plain) is ast.Import and isinstance(plain, ast.stmt)
    assert plain._fields == ("names",) and plain.end_col_offset == 48
    first, second = plain.names
    assert type(first) is ast.alias and isinstance(first, ast.AST)
    assert first._fields == ("name", "asname") and list(ast.walk(plain)) == [plain, first, second]
    assert (first.name, first.asname, first.col_offset, first.end_col_offset) == (
        "_imre_ast_fixture_missing.é", "local", 7, 44
    )
    assert (second.name, second.asname, second.col_offset, second.end_col_offset) == ("π", None, 46, 48)
    assert type(relative) is ast.ImportFrom and isinstance(relative, ast.stmt)
    assert relative._fields == ("module", "names", "level")
    assert (relative.module, relative.level, relative.lineno, relative.end_lineno) == (None, 2, 3, 6)
    assert [(item.name, item.asname, item.lineno) for item in relative.names] == [
        ("first", "other", 4), ("last", None, 5)
    ]
    assert (relative_future.module, relative_future.level) == ("__future__", 1)
    assert (star.module, star.level, star.names[0].name) == ("pkg", 0, "*")


def check_comparison_nodes():
    source = "(\n  é == b != c < d <= e > f >= g\n  is h is not i in j not in k\n)"
    node = ast.parse(source, mode="eval").body
    assert type(node) is ast.Compare and isinstance(node, ast.expr)
    assert node._fields == ("left", "ops", "comparators")
    assert (node.lineno, node.col_offset, node.end_lineno, node.end_col_offset) == (2, 2, 3, 29)
    assert node.left.id == "é" and node.left.end_col_offset == 4
    assert [item.id for item in node.comparators] == list("bcdefghijk")

    kinds = [ast.Eq, ast.NotEq, ast.Lt, ast.LtE, ast.Gt, ast.GtE, ast.Is, ast.IsNot, ast.In, ast.NotIn]
    assert [type(op) for op in node.ops] == kinds
    again = ast.parse(source).body[0].value.ops
    for i, op in enumerate(node.ops):
        assert isinstance(op, ast.cmpop) and op is again[i]
        assert op is not kinds[i]() and not hasattr(op, "lineno")

    built = ast.Compare(node.left, ops=node.ops, comparators=node.comparators)
    assert built.left is node.left and built.ops is node.ops and built.comparators is node.comparators
    empty = ast.Compare(left=node.left)
    empty.ops.append(ast.Eq())
    assert empty.comparators == [] and ast.Compare(left=node.left).ops == []

    try:
        ast.literal_eval("(0 <\n(lambda: 3))")
    except ValueError as error:
        assert str(error).startswith("malformed node or string on line 1: Compare(")
    else:
        raise AssertionError("accepted a comparison as a literal")


def check_template_nodes():
    from ast import Interpolation, TemplateStr

    assert TemplateStr is ast.TemplateStr and Interpolation is ast.Interpolation
    source = '(\n  t"é" t"x{ (ζ) !r:>4}z"\n)'
    node = ast.parse(source, mode="eval").body
    assert type(node) is ast.TemplateStr and isinstance(node, ast.expr)
    assert node._fields == ("values",)
    literal, field, tail = node.values
    assert (literal.value, tail.value) == ("éx", "z")
    assert type(field) is ast.Interpolation and isinstance(field, ast.expr)
    assert field._fields == ("value", "str", "conversion", "format_spec")
    assert (field.value.id, field.str, field.conversion) == ("ζ", " (ζ)", ord("r"))
    assert type(field.format_spec) is ast.JoinedStr
    assert field.format_spec.values[0].value == ">4"
    for child, start, end in ((node, 2, 26), (literal, 4, 11), (field, 11, 24),
                              (field.value, 14, 16), (field.format_spec, 20, 23),
                              (field.format_spec.values[0], 21, 23), (tail, 24, 25)):
        assert (child.lineno, child.col_offset, child.end_lineno, child.end_col_offset) == (2, start, 2, end)
    constructed = ast.Interpolation(field.value, str=None, conversion=-1)
    plain = ast.TemplateStr()
    assert plain.values == [] and plain.values is not ast.TemplateStr().values
    assert constructed.value is field.value and constructed.format_spec is constructed.str is None
    assert ast.TemplateStr([constructed]).values[0] is constructed
    newlines = ast.parse('t"""a\rb\r\nc{value}"""', mode="eval").body.values
    assert newlines[0].value == "a\nb\nc" and newlines[1].lineno == 3
    escaped = ast.parse(r't"�{{\ud800" t"\udfff}}�{x:�\ud800�}"', mode="eval").body.values
    assert [ord(c) for c in escaped[0].value] == [0xfffd, 123, 0xd800, 0xdfff, 125, 0xfffd]
    assert [ord(c) for c in escaped[1].format_spec.values[0].value] == [0xfffd, 0xd800, 0xfffd]
    # A parser that cannot recover CPython's middle-token boundary must reject it.
    continued = 't"""{{' + chr(92) + chr(10) + '"""'
    try:
        literal = ast.parse(continued, mode="eval").body.values[0]
    except NotImplementedError as error:
        assert str(error) == "ast.parse: continued t-string brace literal"
    else:
        assert (literal.value, literal.end_lineno, literal.end_col_offset) == ("{", 1, 6)
    try:
        field = ast.parse(r't"{x:\N{LATIN SMALL LETTER A}x}"', mode="eval").body.values[0]
    except NotImplementedError as error:
        assert str(error) == "ast.parse: named Unicode escape in t-string format spec"
    else:
        assert type(field.format_spec) is ast.Constant and field.format_spec.value == "ax"


def current():
    check_import_nodes()
    check_comparison_nodes()
    check_template_nodes()
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

    root = ReplacingFields()
    root._fields = tuple(["first", "tail"])
    root.tail = ast.Name("retained", ast.Load())
    assert [node.id for node in ast.walk(root) if isinstance(node, ast.Name)] == ["retained"]

    root._fields = CustomFieldOrder(("missing",))
    assert [node.id for node in ast.walk(root) if isinstance(node, ast.Name)] == ["retained"]
