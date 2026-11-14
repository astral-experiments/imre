from xmlrpc.client import Fault, dumps, loads


def current():
    packet = dumps(({"part": b"\x00\xff", "count": 2},), methodname="inventory.put")
    params, method = loads(packet, use_builtin_types=True)
    print(method, params[0]["part"], params[0]["count"])

    try:
        loads(dumps(Fault(17, "invalid <part>")))
    except Fault as exc:
        print(exc.faultCode, exc.faultString)


if __name__ == "__main__":
    current()
