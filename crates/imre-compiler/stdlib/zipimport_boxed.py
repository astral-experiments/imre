from _imre_zipimport import linked_archive as _linked_archive, linked_member as _linked_member, linked_namespace as _linked_namespace, linked_code as _linked_code, contains as _contains, exists as _exists, index as _index, invalidate as _invalidate, isfile as _is_file, read as _read_member


def _read(path, key):
    data = _read_member(path, key)
    if data is None:
        raise OSError(0, '', key)
    return data
