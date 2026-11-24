class Worker
{
    void Start() { Open(); Run(); }

    public static void Stop() { Close(); }

    void Done() { Trace.Enter(nameof(Done)); Close(); }

    int Count() { return 1; }
}
