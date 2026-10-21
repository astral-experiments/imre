import library

library.write()
with open("module.txt", "r", encoding="utf-8") as stream:
    print(stream.read())
print("finished")
