import functools
from functools import WRAPPER_UPDATES
from library import assignments, single, uses_default


for name in assignments():
    print(name)
print(WRAPPER_UPDATES[0])
print(assignments() is functools.WRAPPER_ASSIGNMENTS)
print(assignments(fields=functools.WRAPPER_ASSIGNMENTS) is functools.WRAPPER_ASSIGNMENTS)
print(WRAPPER_UPDATES is functools.WRAPPER_UPDATES)
print(uses_default())
print(uses_default(WRAPPER_UPDATES))
other = single("__dict__")
print(other == WRAPPER_UPDATES)
print(other is not WRAPPER_UPDATES)
print(uses_default(other))
