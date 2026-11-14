# Guide

Wrap with `errors.Wrap` from `github.com/pkg/errors`:

```go
import "github.com/pkg/errors"

return errors.Wrap(err, "read")
```

1. In Rust:

   ```rust
   use old_crate::Thing;
   ```

2. In a shell:

   ```sh
   grep errors.Wrap
   ```
