"""Dataclasses for native runtime-loaded Python classes.

Field construction, parameter metadata and the field queries follow CPython 3.14.5
Lib/dataclasses.py (PSF-2.0). Class methods are compiled closures on the original class.
"""


class _MISSING_TYPE:
    pass


MISSING = _MISSING_TYPE()
