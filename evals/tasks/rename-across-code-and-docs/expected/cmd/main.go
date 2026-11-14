package main

import "example.com/app/config"

func main() {
	cfg, _ := config.LoadConfig("app.toml")
	_ = cfg
}
