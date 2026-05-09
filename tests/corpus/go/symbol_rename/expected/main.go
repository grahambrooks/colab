package main

type NewName struct {
	Field int
}

func (o *NewName) Method() int {
	return o.Field
}

func makeOldName() *NewName {
	return &NewName{Field: 42}
}

func main() {
	v := makeOldName()
	_ = v.Method()
}
