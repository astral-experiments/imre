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
