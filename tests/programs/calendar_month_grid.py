from calendar import firstweekday, monthcalendar, setfirstweekday


def record(order: list[int], value: int) -> int:
    order.append(value)
    return value


for start in range(7):
    setfirstweekday(firstweekday=start)
    print(firstweekday(), monthcalendar(2021, 2), monthcalendar(2021, 5))
for year in [0, -1, 1900, 2000, 10**100, -(10**100)]:
    print(year, monthcalendar(year, 2))

order: list[int] = []
grid = monthcalendar(month=record(order, 2), year=record(order, 2024))
other = monthcalendar(2024, 2)
grid[0][0] = 99
grid[1].append(88)
print(order, grid, other, monthcalendar(2024, 2))

for month in [-1, 0, 13, 10**100]:
    try:
        monthcalendar(2025, month)
    except IndexError as exc:
        print(str(exc))
for start in [-1, 7, 10**100]:
    try:
        setfirstweekday(start)
    except ValueError as exc:
        print(str(exc), firstweekday())
