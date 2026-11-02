import copy
import io
from contextlib import redirect_stdout
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


def debug_compile(source, flags=0):
    output = io.StringIO()
    with redirect_stdout(output):
        compiled = re.compile(source, re.DEBUG | flags)
    return compiled, output.getvalue()


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
    spelling = r'(\.)(?:[ch]|py)(?(1)$|: )'
    quiet = re.compile(spelling)
    first, dump = debug_compile(spelling)
    second, again = debug_compile(spelling)
    assert first is not second and dump and dump == again
    assert first.flags == (quiet.flags | re.DEBUG)
    assert first.groups == 1 and first.fullmatch('.py').group(1) == '.'
    assert first.fullmatch('.h') and first.fullmatch('.py: ') is None
    assert re.compile(spelling) is quiet
    byte_pattern, byte_dump = debug_compile(b'ab', re.I)
    assert byte_dump and byte_dump != dump and byte_pattern.fullmatch(b'AB')
    assert byte_pattern.flags == (re.I | re.DEBUG)
    with redirect_stdout(None):
        assert re.compile('ab', re.DEBUG).fullmatch('ab')

    class Broken:
        def write(self, text):
            raise RuntimeError('debug stream is closed')
    with redirect_stdout(Broken()):
        try:
            re.compile('ab', re.DEBUG)
        except RuntimeError as exc:
            assert str(exc) == 'debug stream is closed'
        else:
            raise AssertionError('debug write error was lost')
    print('pattern cache and comparison passed')
