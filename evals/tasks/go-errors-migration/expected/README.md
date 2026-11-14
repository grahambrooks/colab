# Store

Errors are wrapped with context:

```go
return nil, fmt.Errorf("read config"+": %w", err)
```
