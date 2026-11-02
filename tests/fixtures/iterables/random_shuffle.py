import random
from random import shuffle

# Print the permutation and the subsequent draws for differential comparison with CPython.
random.seed(1729)
values = list(range(12))
alias = values
assert shuffle(x=values) is None
print(alias)
print(random.getrandbits(32))
words = ["red", "blue", "green", "white", "black"]
assert shuffle(words) is None
print(words)
print(random.randrange(1000))
empty: list[int] = []
single = [5]
assert shuffle(empty) is None
assert shuffle(single) is None
print(random.getrandbits(32))
