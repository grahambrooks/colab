import logging
from typing import List, Optional, Tuple

from app.models import Order


def place(items: list[str]) -> tuple[Order, str | None]:
    if not items:
        logging.warning("empty order")
        return Order([]), "empty"
    return Order(items), None


def total(orders: list[Order]) -> int:
    if len(orders) > 100:
        logging.warning("large batch: %d", len(orders))
    return sum(len(o.items) for o in orders)
