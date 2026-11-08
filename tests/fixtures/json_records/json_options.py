import json
text = '"line\nbreak"'
print(json.loads(text, strict=False) == "line\nbreak")
try:
    json.loads(text, strict=True)
except json.JSONDecodeError as error:
    print(type(error).__name__, error.msg)
else:
    print("admitted literal newline")
