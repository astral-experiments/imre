import os


def uuid4() -> UUID:
    """Generate a random UUID."""
    value = int.from_bytes(os.urandom(16))
    value &= ~((0xf << 76) | (3 << 62))
    value |= (4 << 76) | (2 << 62)
    return UUID(int=value)
