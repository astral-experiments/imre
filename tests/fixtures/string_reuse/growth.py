def grow(assignment: bool) -> str:
    text = ""
    for index in range(512):
        if assignment:
            text = text + "é😀" + str(index) + "|"
        else:
            text += "é😀" + str(index) + "|"
    return text


print(grow(False))
print(grow(True))
for seed in ["", "x", "é", "😀", "nul\x00"]:
    text = seed
    for index in range(3):
        text += text
        text = text + ":" + text
        print(text)
    text += ""
    text = text + ""
    print(len(text))
    print(text)

text = "start"
for index in range(8):
    text = text + (text + "!")
print(text)
