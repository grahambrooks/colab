package main

import (
	"fmt"

	"github.com/pkg/errors"
)

type Client struct{}

func NewClient() *Client { return &Client{} }

func main() {
	fmt.Println(NewClient(), errors.New("x"))
}
