import os
from urllib import request
from urllib.request import pathname2url

assert pathname2url is request.pathname2url
assert pathname2url("a b/../λ?#%") == "a%20b/../%CE%BB%3F%23%25"
assert pathname2url("relative", add_scheme=True) == "file:relative"
assert pathname2url("/tmp/a b", add_scheme=True) == "file:///tmp/a%20b"
if os.name == "nt":
    assert pathname2url("//server/share", add_scheme=True) == "file://server/share"
else:
    assert pathname2url("//server/share", add_scheme=True) == "file:////server/share"

try:
    pathname2url("/tmp", True)
except TypeError:
    pass
else:
    raise AssertionError("add_scheme must be keyword-only")

try:
    pathname2url("/tmp", require_scheme=True)
except TypeError:
    pass
else:
    raise AssertionError("pathname2url accepted a url2pathname option")
