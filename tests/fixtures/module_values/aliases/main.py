import library
import library as again
from bridge import read, shared
from library import ALIAS, TABLE

print(library.lookup("é"))
TABLE["a"] = 40
print(read("a"))
shared["b"] = 50
print(again.lookup("b"))
ALIAS["c"] = 60
library.mutate()
for value in library.values():
    print(value)
print(len(library.ROWS[0]))
held = TABLE
held["é"] = 80
print(again.TABLE["é"])
try:
    print(library.read_after_mutation())
except IndexError:
    print("shared list shrank")
