import re


class Text(str):
    pass


class Unhashable(str):
    __hash__ = None


class EqualText(str):
    def __hash__(self):
        return 17

    def __eq__(self, other):
        return True


def compile_fresh(source):
    re.purge()
    return re.compile(EqualText(source))


def current():
    first = re.compile(b'abc')
    assert re.compile(b'abc') is first
    assert re.search(b'abc', b'abc').re is first
    re.purge()
    second = re.compile(b'abc')
    assert first is not second and first == second
    assert hash(first) == hash(second)
    assert {first: 42}[second] == 42
    assert second != re.compile('abc')
    assert second != re.compile(b'abc', re.I)
    literal = re.compile('abc')
    subclass = re.compile(Text('abc'))
    assert subclass is not literal and subclass == literal
    assert re.compile(literal) is literal
    try:
        re.compile(Unhashable('abc'))
    except TypeError:
        pass
    else:
        raise AssertionError('pattern cache accepted an unhashable key')
    plain = compile_fresh('abc')
    grouped = compile_fresh('(?:abc)')
    assert plain == grouped and hash(plain) == hash(grouped)
    assert compile_fresh('(?=a).') != compile_fresh('(?=b).')
    assert compile_fresh('(?P<x>a)') == compile_fresh('(?P<y>a)')
    print('pattern cache and comparison passed')
