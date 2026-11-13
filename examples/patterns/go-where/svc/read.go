package svc

import (
	"os"

	"github.com/pkg/errors"
)

func Read(path, msg string) error {
	f, err := os.Open(path)
	if err != nil {
		return errors.Wrap(err, "open")
	}
	defer f.Close()
	return errors.Wrap(check(f), msg)
}
