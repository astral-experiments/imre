import library
from bridge import SYMBOLS
from library import ALPHABET, DYNAMIC, ORIGINAL, UNICODE, grow, read

text = ORIGINAL
text += " augmented"
print(text)
print(ORIGINAL)
text = read()
text = text + " assigned"
print(text)
print(library.ORIGINAL)
print(grow())
print(read())

encoded = ""
for index in range(4):
    encoded += SYMBOLS[index] + ALPHABET[-index - 1]
encoded += UNICODE[1] + UNICODE[-1] + DYNAMIC[1]
encoded += library.ALPHABET[10]
print(encoded)
for index in [-100, 45, 100000000000000000000]:
    try:
        encoded = encoded + "leading" + SYMBOLS[index]
    except IndexError as problem:
        print(str(problem))
    print(encoded)
