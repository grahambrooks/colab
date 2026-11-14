package main

import (
	"fmt"

	"github.com/pkg/errors"
)

func f() error {
	return errors.Wrap(err, "x")
}
