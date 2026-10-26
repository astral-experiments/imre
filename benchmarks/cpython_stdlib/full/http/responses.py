# verify.py supplies the unchanged CPython responses comprehension above these probes.
print(len(responses))
for member in responses:
    print(HTTPStatus(member.value) is member)
print(responses[HTTPStatus.OK])
print(responses[404])
print(responses[HTTPStatus.REQUEST_ENTITY_TOO_LARGE])
print(200 in responses)
print(999 not in responses)
print(responses.get(404, ("missing", "")))
print(responses.get(999, ("missing", "")))
print(responses.get(999) is None)
responses[HTTPStatus.REQUEST_URI_TOO_LONG] = ("changed", "")
print(len(responses))
print(responses[414])
subset = {HTTPStatus.OK: "ok"}
for code in [404, 999, 10000000000000000000000000]:
    try:
        print(subset[code])
    except KeyError as error:
        print(str(error))
