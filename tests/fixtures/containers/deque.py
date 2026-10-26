from collections import deque

def source() -> list[int]:
    print("source")
    return [1, 2, 3]

def limit_value() -> int:
    print("limit")
    return 2

for limit in range(5):
    queue = deque(range(6), maxlen=limit)
    print(len(queue))
    print(queue.maxlen == limit)
    queue.appendleft(8)
    queue.append(9)
    queue.extendleft([10, 11])
    queue.extend(queue)
    queue.extendleft(queue)
    for item in queue:
        print(item)
    copied = queue.copy()
    queue.clear()
    print(bool(queue))
    for item in reversed(copied):
        print(item)
    while copied:
        print(copied.pop())

text: deque[str] = deque()
text.extend("abc")
text.appendleft("z")
print(text.popleft())
print(text.pop())
for letter in text:
    print(letter)
print(text.maxlen is None)
saved = text.popleft
original = text
text = deque("replacement")
print(saved())
print(original == deque("b", maxlen=3))
print(text != original)
text = original
try:
    for letter in text:
        text.popleft()
except RuntimeError as error:
    print(str(error))
text.clear()
try:
    text.popleft()
except IndexError as error:
    print(str(error))
try:
    deque(maxlen=-1)
except ValueError as error:
    print(str(error))
try:
    deque(maxlen=100000000000000000000000000000000000000000000000000000)
except OverflowError as error:
    print(str(error))

for item in deque(source(), maxlen=limit_value()):
    print(item)
for item in deque(maxlen=limit_value(), iterable=source()):
    print(item)
