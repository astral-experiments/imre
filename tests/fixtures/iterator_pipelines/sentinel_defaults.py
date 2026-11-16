x = b"untouched"
for x in iter(lambda v=b'': v, b''):
    print(x)
print(x)
