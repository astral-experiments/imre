from urllib.request import AbstractDigestAuthHandler

handler = AbstractDigestAuthHandler()

h, kd = handler.get_algorithm_impls("MD5")
assert h('foo') == "acbd18db4cc2f85cedef654fccc4a4d8"
assert kd('foo', 'bar') == "4e99e8c12de7e01535248d2bac85e732"

h, kd = handler.get_algorithm_impls("SHA")
assert h('foo') == "0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33"
assert kd('foo', 'bar') == "54dcbe67d21d5eb39493d46d89ae1f412d3bd6de"

h, kd = handler.get_algorithm_impls("SHA-256")
assert h('foo') == "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"
assert kd('foo', 'bar') == "a765a8beaa9d561d4c5cbed29d8f4e30870297fdfa9cb7d6e9848a95fec9f937"

try:
    handler.get_algorithm_impls("invalid")
except ValueError as exc:
    assert str(exc) == "Unsupported digest authentication algorithm 'invalid'"
else:
    raise AssertionError("accepted unknown algorithm")
