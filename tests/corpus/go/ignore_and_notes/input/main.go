package p

import "github.com/pkg/errors"

func f() error {
	a := errors.Wrap(err, "one")
	// colab:ignore
	b := errors.Wrap(err, "two")
	c := errors.Wrap(err, "three") // colab:ignore pkg-errors
	d := errors.Wrap(err, "four") // colab:ignore other-script
	e := errors.Wrap(err, "five") // colab:ignore go::pattern
	return nil
}
