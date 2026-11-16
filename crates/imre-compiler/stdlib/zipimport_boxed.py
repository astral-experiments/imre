from _imre_zipimport import contains as _contains, exists as _exists, index as _index, invalidate as _invalidate, isfile as _is_file, read as _read_member


def _read(path, key):
    data = _read_member(path, key)
    if data is None:
        raise OSError(0, '', key)
    return data
