from uuid import UUID

first = UUID("00112233-4455-4677-8899-aabbccddeeff")
equal = UUID(int=first.int)
other = UUID(int=0)
tickets = {first: "one", other: "zero"}
print(tickets[equal])
tickets[equal] = "updated"
print(tickets[first])
print(len(tickets))
print(len({first, equal, other}))

one = UUID(int=1)
true = UUID(int=True)
print({one: "one"}[true])
print(hash(true) == hash(true.int))
print(hash(first) == hash(first.int))

# Two different UUID keys can have the same integer hash.
collision = UUID(int=(1 << 61) - 1)
print(hash(collision) == hash(other))
print(len({other, collision}))
maximum = UUID(int=(1 << 128) - 1)
print(hash(maximum))
print(hash(maximum.int))
