# Guide

Wrap with `errors.Wrap` from `github.com/pkg/errors`:

```go
return fmt.Errorf("read"+": %w", err)
```

1. In Rust:

   ```rust
   use new_crate::Thing;
   ```

2. In a shell:

   ```sh
   grep errors.Wrap
   ```
