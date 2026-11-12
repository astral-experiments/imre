class Key(str):
    pass


class Pending:
    def __del__(self):
        print("released", "after" in globals())


globals()[Key("target")] = Pending()
target = after = None
print("done")
