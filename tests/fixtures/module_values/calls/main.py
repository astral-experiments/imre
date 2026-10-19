import library
from library import COUNTER, FIRST

print(FIRST)
print(library.read())
print(library.big())
print(library.ratio())
print(library.byte())
print(COUNTER.value)
COUNTER.value = 99
print(library.COUNTER.value)
