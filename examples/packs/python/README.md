# Python typing for 3.12 (`3.12-typing.colab`)

Annotations written the way 3.9+ and 3.10+ allow, as a reviewable diff:

| Rule | PEP |
| ---- | --- |
| `List` / `Dict` / `Set` / `FrozenSet` / `Tuple` / `Type` `[…]` → `list` / `dict` / `set` / `frozenset` / `tuple` / `type` `[…]` | 585 (3.9) |
| `typing.List[…]` / `typing.Dict[…]` → `list[…]` / `dict[…]` | 585 |
| `Optional[T]` / `typing.Optional[T]` → `T \| None` | 604 (3.10) |
| `Union[A, B]` → `A \| B` | 604 |

The rules apply in annotations and anywhere else the subscript appears
(`cast(Optional[str], x)`), so the code must run on 3.10 or later.

```sh
colab refactor --script examples/packs/python/3.12-typing.colab --format diff .
colab refactor --script examples/packs/python/3.12-typing.colab --write .
ruff check --select F401 --fix .   # the typing imports it leaves unused
```

On the sample in `tests/corpus/python/typing_3_12/`, Python 3.12 imports
the result and `typing.get_type_hints` resolves every annotation.

## What it does not cover

- **Unused `typing` imports.** `from typing import List` stays; a linter
  removes it (ruff F401).
- **`Union` with three or more members**, or nested `Union`s spread over
  lines: only the two-argument form is rewritten.
- **Imports under another name** (`from typing import List as L`,
  `import typing as t`).
- **String annotations** (`"List[int]"`) and type comments: the text is
  a string to the parser.
- **`typing.Callable`, `Iterable` and the other `collections.abc`
  moves**: the import changes module, which needs a person to choose.
- **PEP 695 generics** (`class Box[T]:`, `type` aliases): the change is
  structural, not a rename.
