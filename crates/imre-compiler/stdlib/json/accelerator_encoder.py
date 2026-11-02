from _imre_json import encoder_call as _encoder_call, encoder_state as _encoder_state



def _readonly_encoder_option(index):
    def refuse(*args):
        raise AttributeError("readonly attribute")
    return property(lambda encoder: encoder._state[index], refuse, refuse)


class Encoder:
    """Encoder(markers, default, encoder, indent, key_separator, item_separator, sort_keys, skipkeys, allow_nan)"""

    __slots__ = ('_state',)

    def __new__(cls, /, *args, **kwargs):
        state = _encoder_state(*args, **kwargs)
        self = object.__new__(cls)
        object.__setattr__(self, "_state", state)
        return self

    def __init__(self, /, *args, **kwargs):
        # The native type only initializes state in __new__.
        pass

    def __init_subclass__(cls, **kwargs):
        raise TypeError("type '_json.Encoder' is not an acceptable base type")

    markers = _readonly_encoder_option(0)
    default = _readonly_encoder_option(1)
    encoder = _readonly_encoder_option(2)
    indent = _readonly_encoder_option(3)
    key_separator = _readonly_encoder_option(4)
    item_separator = _readonly_encoder_option(5)
    sort_keys = _readonly_encoder_option(6)
    skipkeys = _readonly_encoder_option(7)

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
