package api

import "github.com/pkg/errors"

func Handle(msg string, err error) error {
	if err != nil {
		return errors.Wrap(err, msg)
	}
	return errors.Wrap(check(), "check")
}

func check() error { return nil }
