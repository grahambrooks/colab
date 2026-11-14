# Store

Errors are wrapped with context:

```go
return nil, errors.Wrap(err, "read config")
```
