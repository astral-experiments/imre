from _imre_zipimport import contains as _has_member, exists as _exists, index as _index, invalidate as _invalidate, isfile as _is_file, read as _read_member


def _current_index(path):
    try:
        return _index(path)
    except OSError:
        return False


def _contains(path, key):
    return _current_index(path) and _has_member(path, key)


def _read(path, key):
    if not _current_index(path):
        raise OSError(0, '', key)
    return _read_member(path, key)
