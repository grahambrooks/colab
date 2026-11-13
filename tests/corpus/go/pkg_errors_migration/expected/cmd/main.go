package main

import (
	"fmt"
)

const usage = "errors.Wrap(err, msg) is gone"

func run() error {
	return fmt.Errorf("%s: %w", "run", check())
}

func check() error { return nil }

func main() {
	fmt.Println(usage, run())
}
