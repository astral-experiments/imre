from inspect import _sentinel as fallback

class Reader:
    fallback = 37

    @classmethod
    def extract(cls, value=fallback):
        return value

print(Reader.extract())
print(Reader.extract(2))
