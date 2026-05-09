package demo;

public class OldGreeter {
    public OldGreeter() {}

    public String greet(String name) {
        return "Hello " + name;
    }
}

class Caller {
    OldGreeter g = new OldGreeter();
}
