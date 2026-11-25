"""A lending library: the property suite's Python sample."""

from __future__ import annotations

import dataclasses
import json
from collections import defaultdict
from datetime import date, timedelta
from enum import Enum
from pathlib import Path
from typing import Iterable, Optional


class Kind(Enum):
    BOOK = "book"
    DVD = "dvd"
    MAGAZINE = "magazine"


@dataclasses.dataclass(frozen=True)
class Title:
    isbn: str
    name: str
    kind: Kind = Kind.BOOK
    tags: tuple[str, ...] = ()

    @property
    def loan_days(self) -> int:
        return {Kind.BOOK: 21, Kind.DVD: 7}.get(self.kind, 14)


@dataclasses.dataclass
class Loan:
    title: Title
    member: str
    due: date
    returned: Optional[date] = None

    def overdue(self, today: date) -> bool:
        return self.returned is None and today > self.due


class LibraryError(Exception):
    """Raised for a request the library refuses."""


class Library:
    def __init__(self, name: str) -> None:
        self.name = name
        self._titles: dict[str, Title] = {}
        self._copies: defaultdict[str, int] = defaultdict(int)
        self._loans: list[Loan] = []

    def __repr__(self) -> str:
        return f"Library({self.name!r}, titles={len(self._titles)})"

    def add(self, title: Title, copies: int = 1) -> None:
        if copies < 1:
            raise LibraryError(f"cannot add {copies} copies of {title.name}")
        self._titles[title.isbn] = title
        self._copies[title.isbn] += copies

    def available(self, isbn: str) -> int:
        out = sum(1 for loan in self._loans if loan.title.isbn == isbn and loan.returned is None)
        return self._copies[isbn] - out

    def lend(self, isbn: str, member: str, today: date) -> Loan:
        title = self._titles.get(isbn)
        if title is None:
            raise LibraryError(f"no title {isbn}")
        if self.available(isbn) <= 0:
            raise LibraryError(f"no copy of {title.name} is free")
        loan = Loan(title, member, today + timedelta(days=title.loan_days))
        self._loans.append(loan)
        return loan

    def give_back(self, loan: Loan, today: date) -> int:
        loan.returned = today
        late = (today - loan.due).days
        return max(0, late) * 25

    def overdue(self, today: date) -> list[Loan]:
        return sorted(
            (loan for loan in self._loans if loan.overdue(today)),
            key=lambda loan: (loan.due, loan.member),
        )

    def by_member(self) -> dict[str, list[str]]:
        names: dict[str, list[str]] = {}
        for loan in self._loans:
            names.setdefault(loan.member, []).append(loan.title.name)
        return names

    def search(self, *words: str, kind: Kind | None = None) -> Iterable[Title]:
        wanted = [w.lower() for w in words]
        for title in self._titles.values():
            if kind is not None and title.kind is not kind:
                continue
            if all(w in title.name.lower() for w in wanted):
                yield title

    def save(self, path: Path) -> None:
        data = {
            "name": self.name,
            "titles": [dataclasses.asdict(t) | {"kind": t.kind.value} for t in self._titles.values()],
            "copies": dict(self._copies),
        }
        path.write_text(json.dumps(data, indent=2))

    @classmethod
    def load(cls, path: Path) -> "Library":
        data = json.loads(path.read_text())
        library = cls(data["name"])
        for raw in data["titles"]:
            title = Title(raw["isbn"], raw["name"], Kind(raw["kind"]), tuple(raw["tags"]))
            library.add(title, data["copies"].get(title.isbn, 1))
        return library


def fine_summary(fines: dict[str, int]) -> str:
    lines = []
    for member, cents in sorted(fines.items(), key=lambda kv: -kv[1]):
        match cents:
            case 0:
                continue
            case n if n < 100:
                lines.append(f"{member}: {n}c")
            case n:
                lines.append(f"{member}: ${n / 100:.2f}")
    return "\n".join(lines) or "no fines"


def demo() -> None:
    library = Library("Main Street")
    library.add(Title("978-0", "The Rust Book", tags=("programming",)), copies=2)
    library.add(Title("978-1", "Night Train", Kind.DVD))
    today = date(2026, 11, 24)
    loan = library.lend("978-0", "ana", today)
    fine = library.give_back(loan, today + timedelta(days=30))
    print(fine_summary({"ana": fine, "bo": 0}))
    assert library.available("978-0") == 2, "the copy came back"


if __name__ == "__main__":
    demo()
