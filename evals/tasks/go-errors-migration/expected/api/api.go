package api

import (
	"fmt"

	"github.com/pkg/errors"
)

func Handle(msg string, err error) error {
	if err != nil {
		return errors.Wrap(err, msg)
	}
	return fmt.Errorf("check"+": %w", check())
}

func check() error { return nil }
