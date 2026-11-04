import codecs
import encodings


registered = codecs.lookup("IDNA")
print(registered is codecs.lookup("-idna-"))
print(registered is encodings.search_function("idna"))
print(encodings.search_function("IDNA") is None)

for spelling in (b"idna", "-idna-", " idna "):
    entry = encodings.search_function(spelling)
    print(entry.name, entry is registered, entry is encodings.search_function(spelling))

print(b"XN--BCHER-KVA.example".decode(encoding="IDNA", errors="strict"))
