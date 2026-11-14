package a

import "github.com/pkg/errors"

func load() error {
	return errors.Wrap(err, "load")
}

func parse() {}
