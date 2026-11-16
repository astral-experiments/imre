class Separator(str):
    pass

s = b'x\xff\xc3\xa9\xffz'.decode('utf-8', 'surrogateescape')
sep = Separator(chr(0xdcff))
left, mid, right = s.partition(sep)
assert left == 'x' and mid is sep and right == 'é' + sep + 'z'
left, mid, right = s.rpartition(sep)
assert left == 'x' + sep + 'é' and mid is sep and right == 'z'
assert s.partition('é') == ('x' + sep, 'é', sep + 'z')
assert s.partition('q')[0] is s and s.rpartition('q')[2] is s
assert 'abc'.partition(sep) == ('abc', '', '')
try:
    s.partition('')
except ValueError as e:
    assert str(e) == 'empty separator'
else:
    raise AssertionError('accepted empty separator')
print('partition hit, reverse hit, separator identity, miss, empty separator')
