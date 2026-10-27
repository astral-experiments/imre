from collections import deque

class Stop(Exception):
    pass

calls = 0
limit = 0

class Key:
    def __eq__(self, other):
        global calls
        calls += 1
        if calls == limit:
            raise Stop()
        return True


def run(kind, n):
    global calls, limit
    calls = 0
    limit = n
    if kind == 'list':
        a = [Key()]; b = [Key()]
        a.append(a); b.append(b)
        a = deque([a]); b = deque([b])
    else:
        a = deque([Key()]); b = deque([Key()])
        if kind == 'mixed':
            a.append([a]); b.append([b])
        else:
            a.append(a); b.append(b)
    try:
        print(kind, a == b, calls)
    except Stop:
        print(kind, 'stop', calls)
    except RecursionError:
        print(kind, 'recursion', calls)


def run_stop_cases():
    run('list', 3)
    run('mixed', 1500)
    run('deque', 1500)


class BreakCycle:
    def __eq__(self, other):
        global calls
        calls += 1
        if calls == 3:
            left[-1] = 1
            right[-1] = 1
        return True


def run_mutation_case():
    global calls, left, right
    run_stop_cases()
    calls = 0
    left = [BreakCycle()]
    right = [BreakCycle()]
    left.append(left)
    right.append(right)
    print('mutated', deque([left]) == deque([right]), calls, left[-1], right[-1])


class Reentrant:
    def __eq__(self, other):
        global calls
        calls += 1
        if calls % 32 == 0:
            return left == right
        return True


def current():
    global calls, left, right
    run_mutation_case()
    calls = 0
    left = deque([Reentrant()])
    right = deque([Reentrant()])
    left.append(left)
    right.append(right)
    try:
        print(left == right)
    except RecursionError:
        print('reentrant', calls > 32)
    run('deque', 1500)
