import importlib.metadata
from importlib.metadata import EntryPoints, entry_points

points = entry_points(group="imre-test")
assert points.groups == {"imre-test"}
assert points.names == {"tool:run"}
ep = points["tool:run"]
assert ep.dist.name == "demo-tools" and ep.dist.version == "1"
assert ep.module == "demo" and ep.attr == "main"
assert ep.extras == ["fast", "compat"]
missing = importlib.metadata.entry_points(group="imre-test", name="absent")
assert points.select(name="absent") == missing
assert isinstance(missing, EntryPoints) and missing == ()
print(ep, repr(missing))
