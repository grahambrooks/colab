# Go up to 1.18 (`1.18-modern.colab`)

The modernisations that are pure syntax, as a reviewable diff:

| Rule | Since |
| ---- | ----- |
| `interface{}` (however spaced) → `any` | Go 1.18 |
| `ioutil.ReadFile` / `WriteFile` → `os.ReadFile` / `os.WriteFile` | Go 1.16 |
| `ioutil.ReadAll` / `NopCloser` / `Discard` → `io.…` | Go 1.16 |
| `ioutil.TempDir` / `TempFile` → `os.MkdirTemp` / `os.CreateTemp` | Go 1.16 |

Each `ioutil` rule adds the `os` or `io` import to the files it changes,
and drops `io/ioutil` once nothing uses it.

```sh
colab refactor --script examples/packs/go/1.18-modern.colab --format diff .
colab refactor --script examples/packs/go/1.18-modern.colab --write --bisect 'go build ./...' .
```

Or name it in `colab.toml` and run `colab fix`:

```toml
[[packs]]
pack = "go/1.18-modern"
```

On the sample in `tests/corpus/go/modern_1_18/`, `go vet` and
`go build` pass before and after.

## What it does not cover

- **`go.mod`'s `go` directive.** `any` needs `go 1.18` or later there;
  bump it yourself.
- **`ioutil.ReadDir`.** `os.ReadDir` returns `[]fs.DirEntry`, not
  `[]fs.FileInfo`, so callers change too.
- **`ioutil` imported under another name** (`import iu "io/ioutil"`), or
  its functions passed as values rather than called.
- **`interface{}` in generated code** you regenerate anyway; exclude it
  with `[run] exclude` or a `colab:ignore` comment.
- **Anything needing types**: `errors.Is` / `As` over `==`, loop variable
  capture (1.22), `min` / `max` builtins (1.21).
