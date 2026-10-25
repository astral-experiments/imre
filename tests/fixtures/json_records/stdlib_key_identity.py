import json


def get_keys(document: str) -> list[str]:
    rows = json.loads(document)
    keys: list[str] = []
    for row in rows:
        for key in row:
            name: str = key
            keys.append(name)
    return keys


source = '[{"a_key":1,"b_é":2},{"a_key":3,"b_\\u00e9":4}]'
first = get_keys(source)
second = get_keys(source)
print(first[0] is first[2])
print(first[1] is first[3])
print(first[0] == second[0])
print(first[0] is not second[0])
print(first[1] is not second[1])

# The outer object is eager. The two nested arrays are indexed and expanded
# independently, while escaped spellings still identify the same Python key.
padding = ',"padding":"' + 'x' * 4200 + '"'
doc = json.loads('{"a_key":0,"b_é":1,"left":[{"a_key":2,"b_é":3' + padding
                 + '}],"right":[{"a_key":4,"b_\\u00e9":5' + padding + '}]}')
root: list[str] = []
left: list[str] = []
right: list[str] = []
for key in doc:
    name: str = key
    root.append(name)
for key in doc["left"][0]:
    name: str = key
    left.append(name)
for key in doc["right"][0]:
    name: str = key
    right.append(name)
print(root[0] is left[0])
print(root[1] is right[1])
print(left[2] is right[2])
