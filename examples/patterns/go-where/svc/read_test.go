package svc

import (
	"testing"

	"github.com/pkg/errors"
)

func TestRead(t *testing.T) {
	if err := errors.Wrap(Read("x", "y"), "in test"); err == nil {
		t.Fatal("expected an error")
	}
}
