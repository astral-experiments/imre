# Run only with a virtual address limit; the requested result is 512 MiB.
source = "xx\t"
try:
    print(len(source.expandtabs(1 << 29)))
except MemoryError as error:
    print(repr(error))
    print(repr(str(error)))
print(repr(source.expandtabs(4)))
