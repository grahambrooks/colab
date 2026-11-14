package main

import "example.com/app/config"

func main() {
	cfg, _ := config.ParseConfig("app.toml")
	_ = cfg
}
