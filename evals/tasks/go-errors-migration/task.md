# Migrate from github.com/pkg/errors to fmt

Replace every `errors.Wrap(err, "msg")` whose message is a literal with
`fmt.Errorf("msg: %w", err)`, outside tests, importing `fmt` and dropping
`github.com/pkg/errors` where nothing uses it any more. Update the Go
examples in the README the same way.
