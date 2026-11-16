import json
from typing import Any

first: dict[str, Any] = json.loads('{"":0,"x":1,"é":2,"中":3,"long key":4}')
second: dict[str, Any] = json.loads(r'{"":0,"\u0078":1,"\u00e9":2,"\u4e2d":3,"long \u006bey":4}')
left = list(first)
right = list(second)
print(left == right)
for i in range(len(left)):
    print(left[i] is right[i])
