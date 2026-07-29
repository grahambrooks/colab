# One script, six languages

A logging library moved and renamed its entry point. In a polyglot repo
that change lands in C, C#, PHP, Ruby, Kotlin, and Swift at once — and it
is exactly the kind of mechanical, structural edit colab exists for.

```sh
# From the repo root. Start with the blast radius:
colab refactor --script examples/polyglot/rename-logger.codemod \
    --format json examples/polyglot
```

```json
{"summary":{"visited":7,"scanned":6,"changed":6,"skipped":0,"elapsed_ms":6},
 "rules":[{"i":0,"files":1,"rule":"c::include \"oldlog/log.h\" -> \"newlog/log.h\""},
          {"i":1,"files":1,"rule":"c::call \"log_write\" -> replace_call \"newlog_write($args)\""},
          … 12 rules, one file each …]}
```

Every rule reports `files: 1`, so nothing is dead. Then:

```sh
colab refactor --script examples/polyglot/rename-logger.codemod --format diff examples/polyglot
colab refactor --script examples/polyglot/rename-logger.codemod --write examples/polyglot
```

## What it shows

**One DSL, twelve rules, six grammars.** Each backend names its import
construct after the language's own — `c::include`, `csharp::using`,
`php::use`, `ruby::require`, `kotlin::import`, `swift::import` — but the
actions (`replace` / `delete` / `ensure` / `replace_call`) are identical
everywhere.

**Rules for other languages cost nothing.** The two C rules are skipped
outright for `Service.kt`, not run and discarded: colab gates each rule on
the path before parsing. That is why a 12-rule polyglot script runs about
as fast as a 2-rule single-language one.

**`visited: 7` vs `scanned: 6`.** The seventh file is the `.codemod`
script itself — visited by the walker, but not a language any rule
targets, so never parsed.

## Per-language notes worth knowing

| Language | Gotcha |
| -------- | ------ |
| C / C++ | The match string is the **bare path**: `"oldlog/log.h"` matches both `#include "oldlog/log.h"` and `#include <oldlog/log.h>`, and rename preserves whichever style the file used. |
| PHP | DSL string literals have **no escape sequences**, so a namespace separator is a single backslash: `"OldLog\Client"`. |
| Ruby | `require` is an ordinary method call, so matching is on the string argument. A computed `require File.join(...)` never matches. |
| Kotlin | For `import a.b.C as D` the target is `a.b.C`, not the alias. A star import `a.b.*` is matched by `"a.b"` — the `*` is punctuation. |
| Swift | Argument labels are part of the argument text, so `$1`/`$2` keep each label with its value when reordering. |
| C# | For `using Alias = Foo.Bar;` the target is the right-hand side, `Foo.Bar`. |

## Call targets include the receiver

`Log.write` and a bare `write` are different targets in every backend
that has `call`. This is deliberate — matching a bare method name against
every receiver would be almost impossible to review. Write the target the
way the call reads in the source:

| Source | Target |
| ------ | ------ |
| `log_write(x)` | `"log_write"` |
| `Log.Write(x)` (C#) | `"Log.Write"` |
| `Log::write($x)` (PHP) | `"Log::write"` |
| `$obj->write($x)` (PHP) | `"$obj->write"` |
| `Log.write(x)` (Ruby/Kotlin/Swift) | `"Log.write"` |

## Idempotency

Every rule here renames its function, so the script is safe to re-run —
the second pass finds no `log_write` left to match. A template that
*keeps* the name (`f` → `f(ctx, $args)`) would wrap again on every run;
apply those with a single `--write` and keep them out of CI.
