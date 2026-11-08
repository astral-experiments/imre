import json
root = json.loads("[]")
root.append(root)
print(len(root), root[0] is root)
