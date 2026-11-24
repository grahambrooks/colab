using System;

public class Service
{
    void Log() {}

    [Obsolete]
    public static void Log() {}

    int Count() { throw new NotImplementedException(); }

    [Obsolete("use Size")]
    public static async Task<int> Length() { throw new NotImplementedException(); }

    protected internal virtual string Name() { throw new NotImplementedException(); }

    private void Reset() { Count(); }

    private static void Clear() { Count(); }
}
