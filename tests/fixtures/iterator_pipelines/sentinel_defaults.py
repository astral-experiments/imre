for x in iter(lambda v=b"": v, b""):
    print(x)
    break
else:
    print("default matched the sentinel")
