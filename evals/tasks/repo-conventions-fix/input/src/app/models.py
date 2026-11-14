from typing import Dict, List, Optional


class Order:
    def __init__(self, items: List[str], notes: Optional[str] = None) -> None:
        self.items = items
        self.notes = notes

    def counts(self) -> Dict[str, int]:
        out: Dict[str, int] = {}
        for item in self.items:
            out[item] = out.get(item, 0) + 1
        return out
