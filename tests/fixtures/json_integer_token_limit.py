import json


for text in ("9" * 640, "-" + "9" * 641, '{"unused": [' + " " * 4096 + "9" * 641 + ']}',
             '"' + "9" * 641 + '"', "1e" + "9" * 641):
    try:
        document = json.loads(text)
        print("accepted")
    except ValueError as error:
        print(str(error))
