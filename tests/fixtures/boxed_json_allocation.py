import json


def current(sort):
    values = {i: i for i in range(1024, -1, -1)}
    expected = json.dumps(values, sort_keys=sort)
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    try:
        json.dumps(values, sort_keys=sort)
    except MemoryError as exc:
        print("caught", exc.args)
    print(len(values), json.dumps(values, sort_keys=sort) == expected)


def output_growth():
    values = [True] * 1231
    marker = {}
    marker["start formatter allocation probe 2332"] = 1
    encoded = json.dumps(values)
    print(len(encoded), len(values), encoded == json.dumps(values))
