package b

import (
	"fmt"
	"log"

	"github.com/pkg/errors"
)

func parse() error {
	log.Print("x", 1)
	fmt.Println()
	return fmt.Errorf("%s: %w", "load", err)
}
