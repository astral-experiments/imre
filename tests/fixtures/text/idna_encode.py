import codecs


def encode(host: str, codec: str) -> bytes:
    return host.encode(codec)


for name in ("example.com", "BÜCHER。de.", "Straße.example", "ca\u00adfé.example"):
    print(encode(name, "idna"))
    print(codecs.lookup("IDNA").encode(name))

for name in ("a..b", "a" * 64, "prefix.\u05d0a", "a\ue000b"):
    try:
        print(encode(name, "idna"))
    except UnicodeEncodeError as error:
        print(error.args)
