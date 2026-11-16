from uuid import UUID


class Guid(UUID):
    @property
    def bytes(self):
        print("read bytes")
        return b"0123456789abcdef"


value = Guid(int=1)
print(str(value))
print(value.bytes_le.hex())
