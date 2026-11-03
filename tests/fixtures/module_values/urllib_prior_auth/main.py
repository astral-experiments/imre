from urllib.request import HTTPPasswordMgrWithDefaultRealm, HTTPPasswordMgrWithPriorAuth

mgr = HTTPPasswordMgrWithPriorAuth()
assert isinstance(mgr, HTTPPasswordMgrWithDefaultRealm)
paths = ["http://sample.test/a", "http://sample.test/b"]
mgr.add_password("area", paths, "user", "pw", is_authenticated=True)
for base in paths:
    assert mgr.is_authenticated(base + "/nested") is True
    assert mgr.is_authenticated(base + "suffix") is None
    assert mgr.find_user_password("other", base) == ("user", "pw")
assert mgr.is_authenticated("http://sample.test:80/a") is True
assert mgr.is_authenticated("http://other.test/a") is None
mgr.update_authenticated(paths[0])
assert mgr.is_authenticated(paths[0]) is False
assert mgr.is_authenticated(paths[1]) is True
