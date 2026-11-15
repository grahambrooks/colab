# One name, every file type: `text::word`

The service `billing` became `invoicing`. Its name is written in a Go
string, a Kubernetes manifest, a Dockerfile, a shell script and the
README. No grammar reaches all of those, and `sed` would also rename
`billingctl`, a separate tool. `text::word` matches the name as a whole
word in any text file:

```
match text::word "billing" { replace "invoicing" }
```

```sh
# From the repo root. Where does it match?
colab find --script examples/text/rename-service.colab examples/text/repo

# The blast radius, then the diff:
colab refactor --script examples/text/rename-service.colab --format json examples/text/repo
colab refactor --script examples/text/rename-service.colab --format diff examples/text/repo
```

```json
{"changed":["examples/text/repo/Dockerfile","examples/text/repo/README.md",
            "examples/text/repo/deploy/rollout.sh","examples/text/repo/deploy/service.yaml",
            "examples/text/repo/src/main.go"],
 "rules":[{"files":5,"i":0,"rule":"text::word \"billing\" -> replace \"invoicing\""}],
 "summary":{"changed":5,"scanned":5,"skipped":0,"visited":5,…}}
```

```diff
-kubectl rollout restart deployment/billing
+kubectl rollout restart deployment/invoicing
 billingctl status
```

## What it shows

**A word, not a substring.** Where the target starts or ends with a
letter, digit or `_`, the character beside it must not be one. So
`billing` matches in `/billing`, `deployment/billing`, `billing:1.4` and
`"billing"`, and not in `billingctl`. `text::literal` takes every
occurrence, `billingctl` included.

**Any text file.** The rule reads every file the walk reaches (hidden
directories and `.gitignore`d paths are skipped, as always, and common
binary formats such as `.png` and `.zip` are not read). Scope it with
`in "<glob>"` or by the paths you give, and preview first.

**No grammar, so no structure.** `text` cannot tell a comment from code
or one `billing` from another that means something else. When the change
lives in one language, prefer that language's `symbol`, `import` or
`pattern` rule; reach for `text` when the same string runs through files
no single grammar covers.

**Dead rules still explain themselves.** A rule that matches nothing
lists what it nearly matched: the target in another case (`Billing`),
and for `word`, the longer words it occurs inside (`billingctl`).
