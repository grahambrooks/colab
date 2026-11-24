package route

func handler(version string) string {
	switch version {
	case "legacy":
		warn("legacy")
		return "old"
	case "v1":
		return "v1"
	default:
		return "next"
	}
}
