"""Verify observed key identity, separate interests and the poll fallback semantics."""
import os
import selectors

r, w = os.pipe()
s = selectors.DefaultSelector()
k = s.register(w, selectors.EVENT_WRITE, 'data')
assert k is s.get_key(w)
assert k[0] == w and k.fd == w and k.fileobj == w and k.events == selectors.EVENT_WRITE
assert k.data == 'data'
for key, mask in s.select(timeout=0):
    assert key is k and mask == selectors.EVENT_WRITE
assert s.modify(w, selectors.EVENT_WRITE, 'data') is k
updated = s.modify(w, selectors.EVENT_READ)
assert updated is not k and updated.data is None
assert len(s.select(-(1 << 1100))) == 0
assert s.unregister(w) is updated
# The host default can be epoll or kqueue. These close-specific observations
# belong to PollSelector, which is the backend exposed by this Imre build.
s.close()
s = selectors.PollSelector()
s.register(r, selectors.EVENT_READ | selectors.EVENT_WRITE)
os.close(w)
assert s.select(-1)[0][1] == selectors.EVENT_READ | selectors.EVENT_WRITE
s.close()
assert len(s.select(0)) == 0
try:
    s.get_key(r)
except RuntimeError:
    pass
else:
    raise AssertionError('closed')
owner = selectors.DefaultSelector()
try:
    with owner as entered:
        assert entered is owner
        entered.register(r, selectors.EVENT_READ)
        raise ValueError('body ran')
except ValueError:
    pass
else:
    raise AssertionError('with suppressed an exception')
try:
    owner.get_key(r)
except RuntimeError:
    pass
else:
    raise AssertionError('exceptional exit did not close')
with selectors.DefaultSelector() as normal:
    normal.register(r, selectors.EVENT_READ)
try:
    normal.get_key(r)
except RuntimeError:
    pass
else:
    raise AssertionError('normal exit did not close')
os.close(r)
print('selector passed')
