import sys
from sys import getprofile, gettrace, setprofile


assert gettrace is sys.gettrace
assert gettrace.__self__ is sys
assert gettrace() is None
sys.gettrace = lambda: 42
try:
    assert sys.gettrace() == 42
    assert gettrace() is None
finally:
    sys.gettrace = gettrace


assert getprofile is sys.getprofile and getprofile.__self__ is sys
assert setprofile is sys.setprofile and setprofile.__self__ is sys
assert getprofile() is None
events = []


def observe_profile(event, args):
    if event == "sys.setprofile":
        events.append(args)


sys.addaudithook(observe_profile)
assert setprofile(None) is None
assert setprofile(None) is None
assert getprofile() is None
assert events == [(), ()]
