import ast
from ast import NodeTransformer, NodeVisitor, iter_child_nodes, iter_fields


def current():
    class Names(NodeVisitor):
        def visit_Name(self, node):
            self.names.append(node.id)
            return node.id

    class Rewrite(NodeTransformer):
        def visit_Expr(self, node):
            if node.value.id == "drop":
                return None
            if node.value.id == "repeat":
                return [node, node]
            return self.generic_visit(node)

        def visit_Name(self, node):
            return ast.Name(id=node.id.upper())

    tree = ast.parse("drop\nrepeat\nrename")
    visitor = Names()
    visitor.names = []
    assert visitor.visit(tree) is None
    assert visitor.names == ["drop", "repeat", "rename"]
    assert visitor.visit(tree.body[0].value) == "drop"
    original = tree.body
    repeated = original[1]
    assert Rewrite().visit(tree) is tree
    assert tree.body is original and original[0] is original[1] is repeated
    assert len(original) == 3 and repeated.value.id == "repeat"
    assert original[2].value.id == "RENAME"

    class DeleteName(NodeTransformer):
        def visit_Name(self, node):
            return None

    expression = ast.parse("name", mode="eval")
    assert DeleteName().visit(expression) is expression
    assert not hasattr(expression, "body")

    class Fields(ast.AST):
        _fields = ("missing", "a", "b")

    node = Fields()
    node.a = 1
    fields = iter_fields(node)
    assert next(fields) == ("a", 1)
    node.b = 2
    assert list(fields) == [("b", 2)]

    fields = iter_fields(node)
    assert next(fields) == ("a", 1)
    assert fields.throw(AttributeError("missing")) == ("b", 2)
    fields.close()
    assert next(fields, None) is None

    assert list(ast.iter_child_nodes(tree)) == tree.body
    node.a = tree.body[0]
    children = iter_child_nodes(node)
    assert next(children) is node.a
    node.b = [1, tree.body[2], "text"]
    assert list(children) == [tree.body[2]]
    node.a = (tree.body[0],)
    del node.b
    assert list(iter_child_nodes(node)) == []
