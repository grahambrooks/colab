package demo

type User struct {
	Name string `json:"old_name" yaml:"keep"`
	Age  int    `json:"age,omitempty"`
}

type Order struct {
	UserName string `json:"old_name"`
}

var unrelated = `json:"old_name"` // raw string literal, not a tag
