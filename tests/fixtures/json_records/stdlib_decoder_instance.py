import json
from json.decoder import JSONDecoder

decoder = json.JSONDecoder()
alias = decoder
second = JSONDecoder()
print(decoder is alias)
print(decoder is not second)
print(decoder.memo is decoder.memo)
print(decoder.memo is not second.memo)
print(repr(decoder.decode(s=" \t[1] \n")))
print(repr(decoder.decode('"é"')))
print(len(decoder.memo))
print(len(json.decoder.JSONDecoder().memo))
memo = decoder.memo
memo["a_key"] = "external value"
print(repr(decoder.decode('{"a_key":315}')))
print(memo["a_key"])
try:
    decoder.decode("\ufeff1")
except json.JSONDecodeError as exc:
    print(exc.msg)
    print(exc.pos)
try:
    json.loads("\ufeff1")
except json.JSONDecodeError as exc:
    print(exc.msg)
print(repr(decoder.decode("false")))

# Compare the live string objects from two calls on the same decoder.
text = '[{"a_key":1,"b_é":2},{"a_key":3,"b_\\u00e9":4}]'
names: list[str] = []
for source in [text, text]:
    for row in decoder.decode(source):
        for key in row:
            name: str = key
            names.append(name)
print(names[0] is names[2])
print(names[1] is names[3])
print(names[4] is names[6])
print(names[5] is names[7])
print(names[0] == names[4] and names[0] is not names[4])
print(names[1] == names[5] and names[1] is not names[5])
