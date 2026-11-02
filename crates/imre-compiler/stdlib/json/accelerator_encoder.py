from _imre_json import encoder_call as _encoder_call, encoder_state as _encoder_state


class Encoder:
    """Encoder(markers, default, encoder, indent, key_separator, item_separator, sort_keys, skipkeys, allow_nan)"""

    __slots__ = ('_state',)

    def __new__(cls, /, *args, **kwargs):
        state = _encoder_state(*args, **kwargs)
        self = object.__new__(cls)
        object.__setattr__(self, "_state", state)
        return self

    def __init_subclass__(cls, **kwargs):
        raise TypeError("type '_json.Encoder' is not an acceptable base type")

    markers = property(lambda self: self._state[0])
    default = property(lambda self: self._state[1])
    encoder = property(lambda self: self._state[2])
    indent = property(lambda self: self._state[3])
    key_separator = property(lambda self: self._state[4])
    item_separator = property(lambda self: self._state[5])
    sort_keys = property(lambda self: self._state[6])
    skipkeys = property(lambda self: self._state[7])

    def __setattr__(self, name, value):
        self.__delattr__(name)

    def __delattr__(self, name):
        if name in ("markers", "default", "encoder", "indent", "key_separator", "item_separator", "sort_keys", "skipkeys"):
            raise AttributeError("readonly attribute")
        raise AttributeError(f"'_json.Encoder' object has no attribute '{name}' and no __dict__ for setting new attributes")

    def __call__(self, /, *args, **kwargs):
        return _encoder_call(self._state, *args, **kwargs)


make_encoder = Encoder
del Encoder
