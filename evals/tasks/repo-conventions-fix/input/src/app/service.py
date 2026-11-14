import logging
from typing import List, Optional, Tuple

from app.models import Order


def place(items: List[str]) -> Tuple[Order, Optional[str]]:
    if not items:
        logging.warn("empty order")
        return Order([]), "empty"
    return Order(items), None


def total(orders: List[Order]) -> int:
    if len(orders) > 100:
        logging.warn("large batch: %d", len(orders))
    return sum(len(o.items) for o in orders)
