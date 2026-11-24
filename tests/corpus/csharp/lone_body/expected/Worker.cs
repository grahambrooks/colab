class Worker
{
    void Start() { Trace.Enter(nameof(Start)); Open(); Run(); }

    public static void Stop() { Trace.Enter(nameof(Stop)); Close(); }

    void Done() { Trace.Enter(nameof(Done)); Close(); }

    int Count() { return 1; }
}
