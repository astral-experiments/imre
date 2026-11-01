import dataclasses
from dataclasses import MISSING, _MISSING_TYPE

assert MISSING is dataclasses.MISSING
assert isinstance(MISSING, _MISSING_TYPE)
assert MISSING is not _MISSING_TYPE()
spelling = repr(MISSING)
assert spelling.startswith("<dataclasses._MISSING_TYPE object at 0x")
assert spelling.endswith(">")
assert spelling == repr(dataclasses.MISSING)
