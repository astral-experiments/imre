ORIGINAL = "exported heap-sized global string with Unicode é😀"
ALPHABET = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:"
UNICODE = "aé😀"


def dynamic() -> str:
    return "dynamic alphabet"


DYNAMIC = dynamic()


def read() -> str:
    return ORIGINAL


def grow() -> str:
    text = ORIGINAL
    text += " local"
    return text
