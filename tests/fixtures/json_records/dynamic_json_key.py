import json

key = "field"
rows = json.loads('[{"field":"ready"}]')
value: str = rows[0][key]
print(value)

# A field that is never selected still goes through the same JSON decoder.
rows = json.loads(r'[{"field":"waiting","unused":"\ud800"}]')
value = rows[0][key]
print(value)
