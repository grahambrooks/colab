# Java 8 → 21 (`8-to-21.codemod`)

The modernisations that are pure syntax, as a reviewable diff:

| Rule | Why |
| ---- | --- |
| `new Integer(x)`, and `Long`, `Short`, `Byte`, `Double`, `Float`, `Boolean`, `Character` → `Integer.valueOf(x)` … | the constructors are deprecated for removal (Java 9, JEP 277) |
| `List<String> xs = new ArrayList<String>(…);` → `new ArrayList<>(…)` | the diamond infers it (Java 7) |
| `xs = new ArrayList<String>(…)` → `new ArrayList<>(…)` | likewise, the target type is known |

```sh
colab refactor --script examples/packs/java/8-to-21.codemod --format diff .
colab refactor --script examples/packs/java/8-to-21.codemod --write --bisect 'mvn -q compile' .
```

On the sample in `tests/corpus/java/modern_21/`, `javac -Xlint:all`
reports 7 deprecation warnings before the pack and none after.

## What it does not cover

- **`valueOf` is not `new`.** `valueOf` may return a cached instance, so
  code comparing boxes with `==`, or locking on them, behaves
  differently. Such code was already wrong; search for it first.
- **The diamond in fields, `var` declarations, method arguments and
  `return`s.** With `var` it would infer `Object`; as an argument it can
  change which overload is chosen. Only typed locals and assignments are
  rewritten.
- **Anonymous classes** (`new Comparator<String>() { … }`) keep their
  type arguments.
- **Language features that need types**: records, switch expressions,
  pattern matching for `instanceof` and `switch`, text blocks, sealed
  classes, `var`.
- **API moves**: `javax.*` → `jakarta.*`, removed modules (JAXB, CORBA).
  Add `java::import` rules for the ones your dependencies force:

  ```
  match java::import "javax.annotation.Nonnull" { replace "jakarta.annotation.Nonnull" }
  ```
