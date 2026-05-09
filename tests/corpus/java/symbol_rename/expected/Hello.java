package demo;

public class NewGreeter {
    public NewGreeter() {}

    public String greet(String name) {
        return "Hello " + name;
    }
}

class Caller {
    NewGreeter g = new NewGreeter();
}
