import json
for value in json.loads('[1, "receipt", null]'):
    print(isinstance(value, str))
