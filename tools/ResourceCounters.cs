// Local, read-only Windows counters for the optional development collector.
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Snowflakes.Tools {
    public sealed class CounterResult {
        public string State = "unavailable";
        public string Code = "not_added";
        public double[] Values = new double[0];
    }
    public sealed class ResourceCounters : IDisposable {
        [DllImport("kernel32.dll")] public static extern uint GetActiveProcessorCount(ushort group);
        [StructLayout(LayoutKind.Sequential)] struct MemoryStatus {
            public uint Length, Load;
            public ulong TotalPhysical, AvailablePhysical, TotalPageFile, AvailablePageFile, TotalVirtual, AvailableVirtual, AvailableExtended;
        }
        [DllImport("kernel32.dll")] static extern bool GlobalMemoryStatusEx(ref MemoryStatus status);
        public static ulong PhysicalMemoryBytes() {
            var status = new MemoryStatus { Length = (uint)Marshal.SizeOf(typeof(MemoryStatus)) };
            return GlobalMemoryStatusEx(ref status) ? status.TotalPhysical : 0;
        }
        [DllImport("pdh.dll", CharSet = CharSet.Unicode)] static extern uint PdhOpenQueryW(string source, UIntPtr user, out IntPtr query);
        [DllImport("pdh.dll", CharSet = CharSet.Unicode)] static extern uint PdhAddEnglishCounterW(IntPtr query, string path, UIntPtr user, out IntPtr counter);
        [DllImport("pdh.dll")] static extern uint PdhCollectQueryData(IntPtr query);
        [DllImport("pdh.dll", CharSet = CharSet.Unicode)] static extern uint PdhGetFormattedCounterArrayW(IntPtr counter, uint format, ref uint size, out uint count, IntPtr buffer);
        [DllImport("pdh.dll")] static extern uint PdhCloseQuery(IntPtr query);
        [StructLayout(LayoutKind.Explicit, Size = 16)] struct Value {
            [FieldOffset(0)] public uint Status;
            [FieldOffset(8)] public double Number;
        }
        [StructLayout(LayoutKind.Sequential)] struct Item { public IntPtr Name; public Value Value; }
        const uint MoreData = 0x800007d2;
        readonly Dictionary<string, IntPtr> counters = new Dictionary<string, IntPtr>();
        readonly Dictionary<string, uint> errors = new Dictionary<string, uint>();
        IntPtr query;
        uint lastCollection;

        public ResourceCounters(int processId, int dwmId) {
            uint code = PdhOpenQueryW(null, UIntPtr.Zero, out query);
            if (code != 0) { lastCollection = code; return; }
            Add("process_3d", @"\GPU Engine(pid_" + processId + @"_*engtype_3D)\Utilization Percentage");
            Add("process_committed", @"\GPU Process Memory(pid_" + processId + @"_*)\Total Committed");
            if (dwmId > 0) Add("dwm_3d", @"\GPU Engine(pid_" + dwmId + @"_*engtype_3D)\Utilization Percentage");
        }
        void Add(string key, string path) {
            IntPtr counter;
            uint code = PdhAddEnglishCounterW(query, path, UIntPtr.Zero, out counter);
            if (code == 0) counters[key] = counter; else errors[key] = code;
        }
        public void Collect() { if (query != IntPtr.Zero) lastCollection = PdhCollectQueryData(query); }
        static CounterResult Failure(string code) { return new CounterResult { Code = code }; }
        public CounterResult Read(string key) {
            if (lastCollection != 0) return Failure("0x" + lastCollection.ToString("x8"));
            IntPtr counter;
            if (!counters.TryGetValue(key, out counter)) {
                uint error;
                return Failure(errors.TryGetValue(key, out error) ? "0x" + error.ToString("x8") : "not_added");
            }
            // Wildcard counters are read as arrays, never summed as percentages.
            // Retry from a zero-sized probe if instances change during the call.
            for (int attempt = 0; attempt < 3; attempt++) {
                uint size = 0, count;
                uint code = PdhGetFormattedCounterArrayW(counter, 0x200 | 0x8000, ref size, out count, IntPtr.Zero);
                if (code != MoreData) return Failure("0x" + code.ToString("x8"));
                if (size == 0 || size > 1024 * 1024) return Failure("buffer_limit");
                IntPtr buffer = Marshal.AllocHGlobal((int)size);
                try {
                    code = PdhGetFormattedCounterArrayW(counter, 0x200 | 0x8000, ref size, out count, buffer);
                    if (code == MoreData) continue;
                    if (code != 0) return Failure("0x" + code.ToString("x8"));
                    int stride = Marshal.SizeOf(typeof(Item));
                    if (count == 0) return Failure("no_instances");
                    if (count > 64 || (ulong)count * (uint)stride > size) return Failure("instance_limit");
                    var values = new List<double>();
                    for (int index = 0; index < count; index++) {
                        var item = (Item)Marshal.PtrToStructure(IntPtr.Add(buffer, index * stride), typeof(Item));
                        if (item.Value.Status > 1 || double.IsNaN(item.Value.Number) || double.IsInfinity(item.Value.Number) || item.Value.Number < 0)
                            return Failure("invalid_instance");
                        values.Add(item.Value.Number);
                    }
                    return new CounterResult { State = "available", Code = "ok", Values = values.ToArray() };
                } finally { Marshal.FreeHGlobal(buffer); }
            }
            return Failure("instances_changed");
        }
        public void Dispose() {
            if (query != IntPtr.Zero) { PdhCloseQuery(query); query = IntPtr.Zero; }
            counters.Clear();
            GC.SuppressFinalize(this);
        }
    }
}
