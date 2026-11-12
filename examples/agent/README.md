# Agent ergonomics (M17)

Four features that save an agent a round trip. Run these from the repo
root after `cargo build -p colab-cli`. None of them writes to disk unless
you add `--write`.

## 1. Inline rules: no script file

```sh
colab refactor -e 'match go::import "github.com/acme/log/v2" { replace "log/slog" }' \
  --format diff examples/agent
```

`-e` is repeatable and the rules run in order. A typo is reported against
the rule as you typed it:

```sh
colab refactor -e 'match go::import "a" { replac "b" }' examples/agent
# parse error at line 1, column 24: unexpected token `replac`; expected one of: ...
```

## 2. Near misses: what was there instead

The rule below names the import without its `/v2` suffix and the call
without its trailing `f`. Neither one matches, and colab says what it saw:

```sh
colab refactor \
  -e 'match go::import "github.com/acme/log" { replace "log/slog" }' \
  -e 'match go::call "log.Print" { replace_call "slog.Info($args)" }' \
  --format json examples/agent
```

```json
"rules":[{"i":0,"files":0,"near":[{"value":"github.com/acme/log/v2","files":1}], ...},
         {"i":1,"files":0,"near":[{"value":"log.Printf","files":1}], ...}]
```

## 3. `colab.test_rule`: try a rule on a snippet over MCP

```sh
colab mcp < examples/agent/test_rule.jsonl
```

The second response contains `"changed":true`, `"matches_expected":true`,
the rewritten `output`, and a `diff`. If you edit `expected` in the file,
you get `"matches_expected":false` plus an `expected_diff`.

## 4. `--help-json`: the CLI as data

```sh
colab refactor --help-json | jq '.args[] | {long, short, values}'
```
