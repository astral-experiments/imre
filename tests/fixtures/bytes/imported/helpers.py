from typing import BinaryIO as File


def copy(stream: File) -> bytes:
    return stream.read()


def encode(text: str) -> bytes:
    return text.encode()
