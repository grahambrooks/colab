package main

import (
	"log"
	"testing"
)

func run() {
	log.Print("starting")
}

func TestRun(t *testing.T) {
	log.Print("in a test")
}

func main() { run() }
