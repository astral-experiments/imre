def show(values: list[int]) -> None:
    for value in values:
        print(value)


first, second = [1, 2]
print(first)
print(second)
first, *middle, last = [3, 4, 5, 6]
print(first)
show(middle)
print(last)
first, *middle = [7]
print(first)
print(len(middle))
*middle, last = [8]
print(len(middle))
print(last)
(*middle,) = []
print(len(middle))
[] = []
() = ""
[] = b""

initial, *characters, final = "aé🙂z"
print(initial)
for character in characters:
    print(character)
print(final)
alpha, beta = "é🙂"
print(alpha)
print(beta)

byte_first, *byte_middle, byte_last = b"\x00\x80\xff"
print(byte_first)
show(byte_middle)
print(byte_last)
byte_first, byte_last = b"AB"
print(byte_first)
print(byte_last)

number, *words, flag = (9, "ten", "eleven", True)
print(number)
for word in words:
    print(word)
print(flag)
words = ["discarded"]
number, *words, last = (12, 13)
print(number)
print(len(words))
print(last)

original = [[14], [15], [16]]
head, *tail = original
tail[0].append(17)
tail.append([18])
print(len(original))
print(len(tail))
print(original[1][1])
head.append(19)
print(original[0][1])

numbers = [20, 21, 22]
first, *numbers = numbers
print(first)
show(numbers)
numbers.append(23)
print(len(numbers))

left, (center, right) = (24, (25, 26))
print(left + center + right)

big_first, *big_rest = [
    1000000000000000000000000000000,
    -2000000000000000000000000000000,
]
print(big_first)
print(big_rest[0])
float_first, *float_rest = [-0.0, float("nan"), float("inf")]
print(float_first)
for floating in float_rest:
    print(floating)
