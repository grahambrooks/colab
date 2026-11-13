package svc

import (
	"fmt"
	"os"

	"github.com/pkg/errors"
)

func Read(path, msg string) error {
	f, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open"+": %w", err)
	}
	defer f.Close()
	return errors.Wrap(check(f), msg)
}
