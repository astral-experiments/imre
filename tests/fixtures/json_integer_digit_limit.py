import json


for number in (10**639, -(10**640)):
    try:
        print(len(json.dumps(number)))
    except ValueError as error:
        print(str(error))
    try:
        print(len(json.dumps({number: 0})))
    except ValueError as error:
        print(str(error))
