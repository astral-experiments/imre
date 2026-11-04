from _codecs import _imre_native_search as search_function


# The default provider's entries survive codec-registry cache invalidation.
_module_entries = {}


# CPython's encodings search cache keys the original spelling and is bounded.
def _module_entry(spelling, module):
    entry = _module_entries.get(spelling)
    if entry is None:
        entry = module.getregentry()
        if len(_module_entries) >= 500:
            _module_entries.clear()
        _module_entries[spelling] = entry
    return entry
