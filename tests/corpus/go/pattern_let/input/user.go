package user

type User struct {
	name  string
	email string
}

func (u *User) GetName() string { return u.name }

func (u *User) GetEmail() string {
	return u.email
}

// Getaway is not a getter.
func (u *User) Getaway() string { return "beach" }

func Describe(u *User) string {
	return u.GetName() + " <" + u.GetEmail() + "> " + u.Getaway()
}
