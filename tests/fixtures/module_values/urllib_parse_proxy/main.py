from urllib.request import _parse_proxy

assert _parse_proxy('proxy.example') == (None, None, None, 'proxy.example')
assert _parse_proxy('proxy.example:3128') == (None, None, None, 'proxy.example:3128')
assert _parse_proxy('http://proxy.example/') == ('http', None, None, 'proxy.example')
assert _parse_proxy('http://proxy.example:3128/path') == ('http', None, None, 'proxy.example:3128')
assert _parse_proxy('user:pass@proxy.example:3128') == (None, 'user', 'pass', 'proxy.example:3128')
assert _parse_proxy('http://user@proxy.example/') == ('http', 'user', None, 'proxy.example')
assert _parse_proxy('http://user:p%40ss@proxy.example:3128/path') == ('http', 'user', 'p%40ss', 'proxy.example:3128')
assert _parse_proxy('http://us/er:pa/ss@localhost:22') == ('http', 'us/er', 'pa/ss', 'localhost:22')

try:
    _parse_proxy("file:/proxy.example")
except ValueError as exc:
    assert str(exc) == "proxy URL with no authority: 'file:/proxy.example'"
else:
    raise AssertionError("accepted a proxy URL without an authority")
