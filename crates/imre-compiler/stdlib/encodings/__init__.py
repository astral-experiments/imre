from _codecs import _imre_native_search as search_function


# The default provider's entries survive codec-registry cache invalidation.
_wide_entries = {}


# CPython's encodings search cache keys the original spelling and is bounded.
def _wide_entry(spelling, module):
    entry = _wide_entries.get(spelling)
    if entry is None:
        entry = module.getregentry()
        if len(_wide_entries) >= 500:
            _wide_entries.clear()
        _wide_entries[spelling] = entry
    return entry
