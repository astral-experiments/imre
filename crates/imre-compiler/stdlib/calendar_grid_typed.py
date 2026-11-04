"""Checked layouts for the ordinary integer month grid methods."""
from _imre_calendar import _monthgrid


class Calendar:
    def __init__(self, firstweekday: int = 0) -> None:
        self._firstweekday = firstweekday

    def getfirstweekday(self) -> int:
        return self._firstweekday % 7

    def setfirstweekday(self, firstweekday: int) -> None:
        self._firstweekday = firstweekday

    @property
    def firstweekday(self) -> int:
        return self.getfirstweekday()

    @firstweekday.setter
    def firstweekday(self, value: int) -> None:
        self.setfirstweekday(value)

    def monthdayscalendar(self, year: int, month: int) -> list[list[int]]:
        return _monthgrid(year, month, self._firstweekday)


class TextCalendar(Calendar):
    pass
