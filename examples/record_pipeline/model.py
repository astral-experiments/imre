class Record:
    identifier: int
    amount: int
    risk: int

    def __init__(self, identifier: int, amount: int, risk: int) -> None:
        self.identifier = identifier
        self.amount = amount
        self.risk = risk
