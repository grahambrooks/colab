# App

## Configuration

Call `config.ParseConfig` with the file's path:

```go
cfg, err := config.ParseConfig("app.toml")
```

`ParseConfig` returns a zero `Config` when the file is empty.
