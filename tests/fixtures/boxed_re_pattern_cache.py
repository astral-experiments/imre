import copy
import pickle
import re


class Text(str):
    pass


class Binary(bytes):
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
    binary = Binary(b'abc')
    assert re.compile(binary).pattern is binary and re.fullmatch(binary, b'abc')
    high = compile_fresh(chr(0xd800))
    assert high != compile_fresh(chr(0xdfff))
    assert high.fullmatch(chr(0xd800)) and high.fullmatch(chr(0xdfff)) is None
    p = re.compile(b'a(.)', re.I)
    for proto in range(pickle.HIGHEST_PROTOCOL + 1):
        data = pickle.dumps(p, proto)
        re.purge()
        restored = pickle.loads(data)
        assert restored == p and restored is not p
        assert restored.fullmatch(b'Ab').groups() == (b'b',)
        assert restored.fullmatch(b'bb') is None
    for obj in (p, p.match(bytearray(b'Ab'))):
        cls = type(obj)
        assert copy.copy(obj) is copy.deepcopy(obj) is obj
        assert cls.__copy__.__objclass__ is cls and cls.__copy__(obj) is obj
        assert obj.__copy__.__qualname__ == cls.__name__ + ".__copy__"
        assert cls.__deepcopy__.__get__(obj, cls)(None) is obj
        assert copy.deepcopy(obj, {id(obj): p}) is p
        try:
            cls.__copy__(0)
        except TypeError:
            pass
        else:
            raise AssertionError('copy descriptor accepted a wrong receiver')
    print('pattern cache and comparison passed')
