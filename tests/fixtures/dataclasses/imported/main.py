from facade import Entry


row = Entry.named("aliased")
row.values.append(4)
print(row.name)
print(row.count)
print(row)
