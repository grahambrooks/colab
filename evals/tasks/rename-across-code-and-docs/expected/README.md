# App

## Configuration

Call `config.LoadConfig` with the file's path:

```go
cfg, err := config.LoadConfig("app.toml")
```

`LoadConfig` returns a zero `Config` when the file is empty.
