import library
from library import value as original
import transition
from library import value as current

print(original, current, current is library.value, original is current)
