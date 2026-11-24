using System;

public class Service
{

    int Count() { throw new NotSupportedException(); }

    [Obsolete("use Size")]
    public static async Task<int> Length() { throw new NotSupportedException(); }

    protected internal virtual string Name() { throw new NotSupportedException(); }

    void Reset() { Count(); }

    private static void Clear() { Count(); }
}
