"""Compile a literal HTML document containing one loop over string mappings."""

from jinja2 import nodes


def write_renderer(environment, source, target):
    environment.compile_text("")  # Validate the frozen environment's escape policy.
    tree = environment.parse(source)
    loops = [node for node in tree.body if isinstance(node, nodes.For)]
    if len(loops) != 1:
        raise ValueError("expected one top-level for loop")
    loop = loops[0]
    if (not isinstance(loop.target, nodes.Name) or not isinstance(loop.iter, nodes.Name)
            or loop.else_ or loop.test or loop.recursive):
        raise ValueError("expected an unconditional loop over one sequence")

    def outputs(group):
        for node in group:
            if not isinstance(node, nodes.Output):
                raise ValueError("expected literal output or a loop item field")
            yield from node.nodes

    def literal(group):
        parts = []
        for node in outputs(group):
            if not isinstance(node, nodes.TemplateData):
                raise ValueError("only literal text may surround the loop")
            parts.append(node.data)
        return "".join(parts)

    literals = [""]
    names = []
    for child in outputs(loop.body):
        if isinstance(child, nodes.TemplateData):
            literals[-1] += child.data
        elif (isinstance(child, nodes.Getattr) and isinstance(child.node, nodes.Name)
              and child.node.name == loop.target.name and not hasattr(dict, child.attr)):
            names.append(child.attr)
            literals.append("")
        else:
            raise ValueError("expected a string field on the current loop item")
    index = tree.body.index(loop)
    before = literal(tree.body[:index])
    after = literal(tree.body[index + 1:])
    target.write_text(
        "from jinja2.textprogram import TextProgram\n\n"
        "def render(rows: list[dict[str, str]]) -> str:\n"
        f"    program = TextProgram({literals!r}, {names!r})\n"
        f"    parts = [{before!r}]\n"
        "    for row in rows:\n"
        "        parts.append(program.render(row))\n"
        f"    parts.append({after!r})\n"
        '    return "".join(parts)\n'
    )
