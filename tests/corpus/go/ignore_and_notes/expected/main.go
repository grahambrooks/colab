package p

import (
	"fmt"

	"github.com/pkg/errors"
)

func f() error {
	a := fmt.Errorf("one"+": %w", err)
	// colab:ignore
	b := errors.Wrap(err, "two")
	c := errors.Wrap(err, "three") // colab:ignore pkg-errors
	d := fmt.Errorf("four"+": %w", err) // colab:ignore other-script
	e := errors.Wrap(err, "five") // colab:ignore go::pattern
	return nil
}
