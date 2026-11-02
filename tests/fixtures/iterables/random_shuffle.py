import random
from random import shuffle

# Print the permutation and the subsequent draws for differential comparison with CPython.
random.seed(1729)
values = list(range(12))
alias = values
print(shuffle(x=values), alias, random.getrandbits(32))
words = ["red", "blue", "green", "white", "black"]
print(shuffle(words), words, random.randrange(1000))
empty: list[int] = []
single = [5]
print(shuffle(empty), shuffle(single), random.getrandbits(32))
