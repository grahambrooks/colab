import typing
from typing import Dict, FrozenSet, List, Optional, Set, Tuple, Type, Union


def group(rows: List[Tuple[str, int]]) -> Dict[str, List[int]]:
    out: Dict[str, List[int]] = {}
    for name, n in rows:
        out.setdefault(name, []).append(n)
    return out


def find(names: typing.List[str], want: Optional[str]) -> Union[int, None]:
    if want is None:
        return None
    return names.index(want)


def pair() -> Optional[Dict[str, int]]:
    return None


def kinds(seen: Set[str], frozen: FrozenSet[int], cls: Type[Exception]) -> typing.Dict[str, typing.Optional[int]]:
    return {name: (len(frozen) if cls is ValueError else None) for name in seen}
