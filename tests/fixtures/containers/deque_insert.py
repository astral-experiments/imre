from collections import deque

queue = deque([10, 30], maxlen=4)
queue.insert(1, 20)
insert = queue.insert
insert(-1, 25)
print(list(queue))

cursor = iter(queue)
try:
    insert(0, 5)
except IndexError as exc:
    print(str(exc))
print(list(queue), list(cursor))
