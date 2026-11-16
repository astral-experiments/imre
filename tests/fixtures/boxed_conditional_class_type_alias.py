def owner(enabled):
    class Owner:
        def __getstate__(self):
            return ()

        element = int
        if enabled:
            if element is int:
                type Row[T] = tuple[T, element]

    return Owner


assert "Row" not in owner(False).__dict__
enabled = owner(True)
enabled.element = bytes
parameter, = enabled.Row.__type_params__
assert enabled.Row.__value__ == tuple[parameter, bytes]
print("conditional class alias")
