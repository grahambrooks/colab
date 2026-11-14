package main

import (
	"log"
	"log/slog"
	"testing"
)

func run() {
	slog.Info("starting")
}

func TestRun(t *testing.T) {
	log.Print("in a test")
}

func main() { run() }
