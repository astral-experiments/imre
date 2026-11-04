import codecs

encoder = codecs.getincrementalencoder("idna")()
print(encoder.encode("bü"))
print(encoder.encode("cher。"))
print(encoder.encode("example", final=True))

for encode in (codecs.lookup("idna").encode, codecs.getincrementalencoder("idna")().encode):
    try:
        print(encode("x.\udfff."))
    except UnicodeEncodeError as error:
        print(error.args)
