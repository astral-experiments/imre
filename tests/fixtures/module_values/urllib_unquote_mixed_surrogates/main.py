from urllib.parse import unquote

s = chr(0xdcf0)
assert unquote('/b%61d/' + s) == '/bad/' + s
assert unquote(s + '%F0', errors='surrogateescape') == s * 2
assert unquote(s + '%FF', encoding=None, errors=None) == s + '�'
assert unquote('%61%00' + s + 'bc', encoding='utf-16-le') == 'a' + s + chr(0x6362)
try:
    unquote(s + '%FF', errors='strict')
except UnicodeDecodeError as exc:
    assert exc.object == b'\xff' and exc.start == 0
else:
    raise AssertionError('invalid byte decoded')
print('surrogate literal, percent escape, default, strict and multibyte codec')
