from typing import Dict, List, Optional


class Order:
    def __init__(self, items: list[str], notes: str | None = None) -> None:
        self.items = items
        self.notes = notes

    def counts(self) -> dict[str, int]:
        out: dict[str, int] = {}
        for item in self.items:
            out[item] = out.get(item, 0) + 1
        return out
