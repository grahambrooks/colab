package main

import (
	"fmt"

	"github.com/pkg/errors"
)

const usage = "errors.Wrap(err, msg) is gone"

func run() error {
	return errors.Wrap(check(), "run")
}

func check() error { return nil }

func main() {
	fmt.Println(usage, run())
}
