from re import findall


def check(text: str):
    pairs = findall(r"([a-z]+)=([0-9]+)", text)
    same = pairs[0] is pairs[0]
    distinct = pairs[0] is pairs[1]
    print(same)
    print(distinct)
    first = pairs[0]
    pairs[0] = pairs[1]
    print(first)
    print(pairs)


check("name=1 name=1")
