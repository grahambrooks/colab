import typing
from typing import Dict, List, Optional, Tuple, Union


def group(rows: list[tuple[str, int]]) -> dict[str, list[int]]:
    out: dict[str, list[int]] = {}
    for name, n in rows:
        out.setdefault(name, []).append(n)
    return out


def find(names: list[str], want: str | None) -> int | None:
    if want is None:
        return None
    return names.index(want)


def pair() -> dict[str, int] | None:
    return None
