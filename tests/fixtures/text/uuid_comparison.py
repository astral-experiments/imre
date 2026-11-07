from uuid import UUID

one = UUID(int=1)
truth = UUID(int=True)
print(one == truth)
print(one != truth)
print(one is truth)
print(one == 1)
print(1 != one)

large = UUID(int=(1 << 127) + 3)
copy = UUID(hex=large.hex)
print(large == copy == large)
print(large != UUID(int=(1 << 127) + 4))
