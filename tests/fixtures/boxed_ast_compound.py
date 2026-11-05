"""Compare the structure and source positions exposed to Python AST visitors."""

import ast


def record(value):
    if isinstance(value, ast.AST):
        fields = [(name, record(child)) for name, child in ast.iter_fields(value)]
        position = tuple(getattr(value, name) for name in value._attributes)
        return type(value).__name__, fields, position
    if isinstance(value, list):
        return [record(item) for item in value]
    return value


def current():
    source = """\
@decorate(
    choice)
class Café(Base, *bases, metaclass=Meta, **options):
    @wrap
    async def run(self, stream):
        async with resource() as value, guard:
            async for first, *rest in stream:
                self.data[first:stop:step] += [x for x in rest if x and test(x)]
            else:
                return None

def choose(items):
    for item in items:
        try:
            with item:
                assert item, 'required'
                yield from (convert(x) for x in item if x or fallback)
        except Error as exc:
            raise Other from exc
        except:
            raise
        else:
            continue
        finally:
            del item.cache[:]
    while ready:
        break
    else:
        done()

try:
    collect(key for key in keys)
except* Error:
    pass

lookups = ({k: v for k, v in pairs}, {name for name in names})
render = lambda val: 'prefix ' f'{val = !s:>{width}}' ' suffix'
"""
    tree = ast.parse(source)
    print(record(tree))
    assert ast.parse("a and b", mode="eval").body.op is ast.parse("c and d", mode="eval").body.op
    assert ast.parse("del x").body[0].targets[0].ctx is ast.parse("del y").body[0].targets[0].ctx

    for source in (
        '''f"{'#'=}"''',
        "f'''{('é#' # comment\n)=}'''",
        '''f"{a:{b:{f'{c:{d:{e}}}'}}}"''',
    ):
        print(record(ast.parse(source, mode="eval")))
    try:
        ast.parse('f"{a:{a:{a:{a:}}}}"', mode="eval")
    except SyntaxError:
        pass
    else:
        raise AssertionError("four nested replacement fields should be rejected")

    # Record both annotation forms and the assignment expression's Store target.
    source = """\
from __future__ import annotations as hints
café: list[int] = (count := 1)
(local): T
item.field: T = value
items[0]: T
"""
    print(record(ast.parse(source)))
