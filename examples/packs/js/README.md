# CommonJS → ES modules (`esm.colab`)

The top-level CommonJS forms that have a one-to-one ES module spelling,
as a reviewable diff:

| CommonJS | ES module |
| -------- | --------- |
| `const x = require('m');` | `import x from 'm';` |
| `module.exports = x;` | `export default x;` |
| `exports.name = value;` | `export const name = value;` |

A form inside a block (a function body, an `if`) is left alone: `import`
and `export` are only allowed at the top level.

```sh
colab refactor --script examples/packs/js/esm.colab --format diff .
colab refactor --script examples/packs/js/esm.colab --write --bisect 'npm test' .
```

The sample in `tests/corpus/js/esm/` was checked by parsing the result
(no syntax errors), not by running it under Node.

## What it does not cover

- **Turning the package into ESM.** Set `"type": "module"` in
  `package.json`, or rename files to `.mjs`.
- **Relative specifiers.** ESM needs the extension: `'./helpers'` must
  become `'./helpers.js'`, which depends on the file layout.
- **Destructuring** (`const { a, b } = require('m')`): it becomes a named
  import only when `m` has named exports, which needs the module.
- **Reads of `exports.name`** after the export: they must become `name`.
  `colab find -e "match js::pattern 'exports.\$N' { delete }" .` lists
  them.
- **`require` inside functions and conditionals**, which need
  `await import()`, and **`__dirname` / `__filename`**, which become
  `import.meta.dirname` / `import.meta.filename` (Node 20.11+).
- **A file mixing `module.exports = x` with `exports.y = …`**: the result
  has a default and named exports, which a CommonJS caller sees
  differently.
