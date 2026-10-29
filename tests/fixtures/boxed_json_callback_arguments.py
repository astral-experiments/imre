import json


def current(mode):
    if mode >= 3:
        class Sink:
            pass

        sink = Sink()
        sink.parts = []
        sink.write = sink.parts.append
        payload = True if mode == 3 else {"k": True}
        expected = 'true' if mode == 3 else '{"k": true}'
        marker = 'arm JSON default argument refusal'
        try:
            json.dump(payload, sink, sort_keys=True)
            print('ok', ''.join(sink.parts) == expected)
        except MemoryError as error:
            print('MemoryError', error.args == ())
        sink.parts.clear()
        json.dump(payload, sink, sort_keys=True)
        print(''.join(sink.parts) == expected)
        return

    class Item:
        pass


    item = Item() if mode == 0 else float('nan')
    result = 17
    payload = {item: result} if mode == 2 else item


    def default(value):
        assert value is item
        return result


    try:
        if mode == 0:
            marker = 'arm JSON default argument refusal'
            encoded = json.dumps(payload, default=default)
            print('ok', encoded == '17')
        else:
            marker = 'arm JSON default argument refusal'
            json.dumps(payload, allow_nan=False)
    except MemoryError as error:
        print('MemoryError', error.args == ())
    except ValueError as error:
        print('ValueError', str(error) == 'Out of range float values are not JSON compliant: nan')

    if mode == 0:
        print(json.dumps(payload, default=default) == '17')
    else:
        try:
            json.dumps(payload, allow_nan=False)
        except ValueError as error:
            print(str(error) == 'Out of range float values are not JSON compliant: nan')
