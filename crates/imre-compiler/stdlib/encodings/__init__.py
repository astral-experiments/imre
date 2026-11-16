import codecs
from _codecs import _imre_bind_module_search


class CodecRegistryError(LookupError, SystemError):
    pass


# The default provider's entries survive codec-registry cache invalidation.
_module_entries = {}
_unknown = object()


def _module_entry(spelling, module_name):
    entry = _module_entries.get(spelling, _unknown)
    if entry is not _unknown:
        return entry

    # Only the native resolver selects module_name. A cached spelling survives
    # replacement of the provider or the package in sys.modules.
    try:
        module = __import__(module_name, fromlist=['*'])
    except ImportError:
        module = None

    # Keep the factory alive through cache insertion. If it replaces itself,
    # its destructor can look the same spelling up again.
    try:
        factory = module.getregentry
    except AttributeError:
        entry = None
    else:
        entry = factory()
        if not isinstance(entry, codecs.CodecInfo):
            # CPython 3.14.5 Lib/encodings/__init__.py accepts the old module factory
            # protocol here, before caching. Registered search functions return 4-tuples
            # directly to the codec registry and do not pass through this conversion.
            if not 4 <= len(entry) <= 7:
                raise CodecRegistryError('module "%s" (%s) failed to register'
                                         % (module.__name__, module.__file__))
            if (not callable(entry[0]) or not callable(entry[1])
                    or (entry[2] is not None and not callable(entry[2]))
                    or (entry[3] is not None and not callable(entry[3]))
                    or (len(entry) > 4 and entry[4] is not None and not callable(entry[4]))
                    or (len(entry) > 5 and entry[5] is not None and not callable(entry[5]))):
                raise CodecRegistryError('incompatible codecs in module "%s" (%s)'
                                         % (module.__name__, module.__file__))
            if len(entry) < 7 or entry[6] is None:
                entry += (None,) * (6 - len(entry)) + (module.__name__.split(".", 1)[1],)
            entry = codecs.CodecInfo(*entry)

    if len(_module_entries) >= 500:
        _module_entries.clear()
    _module_entries[spelling] = entry
    return entry


search_function = _imre_bind_module_search(_module_entry)
