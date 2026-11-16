def publish() -> str:
    if False:
        global Child

    class Child:
        pass

    return Child.__qualname__


print(publish())
print(Child.__qualname__)
