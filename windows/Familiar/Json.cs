using System.Text.Json.Nodes;
using Windows.UI;

namespace Familiar;

/// <summary>Reading the engine's JSON without a type for every shape. A missing or mistyped
/// value reads as empty rather than throwing.</summary>
internal static class Json
{
    private static JsonNode? At(JsonNode? node, string key) => (node as JsonObject)?[key];

    public static string? OptStr(this JsonNode? node, string key) =>
        At(node, key) is JsonValue v && v.TryGetValue<string>(out var s) ? s : null;

    public static string Str(this JsonNode? node, string key) => node.OptStr(key) ?? "";

    public static double? OptNum(this JsonNode? node, string key) =>
        At(node, key) is JsonValue v && v.TryGetValue<double>(out var d) ? d : null;

    public static double Num(this JsonNode? node, string key) => node.OptNum(key) ?? 0;

    public static int Int(this JsonNode? node, string key) => (int)node.Num(key);

    public static long Long(this JsonNode? node, string key) =>
        At(node, key) is JsonValue v && v.TryGetValue<long>(out var n) ? n : (long)node.Num(key);

    public static bool Bool(this JsonNode? node, string key) =>
        At(node, key) is JsonValue v && v.TryGetValue<bool>(out var b) && b;

    public static bool IsNull(this JsonNode? node, string key) => At(node, key) is null;

    public static JsonNode? Get(this JsonNode? node, string key) => At(node, key);

    public static IList<JsonNode?> Arr(this JsonNode? node, string key) =>
        At(node, key) is JsonArray a ? a : [];

    public static IList<JsonNode?> Items(this JsonNode? node) => node is JsonArray a ? a : [];

    /// <summary>Arguments for a call. Nodes that already belong to a document are copied, since
    /// a node can only have one parent.</summary>
    public static JsonObject Args(params (string Key, JsonNode? Value)[] pairs)
    {
        var o = new JsonObject();
        foreach (var (key, value) in pairs) o[key] = value?.Parent is null ? value : value.DeepClone();
        return o;
    }

    /// <summary>"#rrggbb". Anything else is null.</summary>
    public static Color? Hex(string? text)
    {
        if (text is null || text.Length != 7 || text[0] != '#') return null;
        return uint.TryParse(text.AsSpan(1), System.Globalization.NumberStyles.HexNumber, null, out var v)
            ? Color.FromArgb(255, (byte)(v >> 16), (byte)(v >> 8), (byte)v)
            : null;
    }
}
