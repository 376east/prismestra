using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;

public sealed class PrismestraCiDdpReceiver : IDisposable {
    [DllImport("iphlpapi.dll")] static extern uint GetExtendedUdpTable(IntPtr table, ref uint size,
        bool order, uint family, int tableClass, uint reserved);
    [DllImport("iphlpapi.dll")] static extern uint GetExtendedTcpTable(IntPtr table, ref uint size,
        bool order, uint family, int tableClass, uint reserved);

    // Adapted from the hosted recovery stand. Zero means no owner and -1 means
    // an ambiguous owner or a non-loopback SDK listener.
    static int Owner(int port, bool tcp) {
        uint size = 0;
        uint code = tcp ? GetExtendedTcpTable(IntPtr.Zero, ref size, false, 2, 3, 0)
                        : GetExtendedUdpTable(IntPtr.Zero, ref size, false, 2, 1, 0);
        for (int attempt = 0; code == 122 && size >= 4 && size <= 16 * 1024 * 1024 && attempt < 4; attempt++) {
            IntPtr table = Marshal.AllocHGlobal((int)size);
            try {
                code = tcp ? GetExtendedTcpTable(table, ref size, false, 2, 3, 0)
                           : GetExtendedUdpTable(table, ref size, false, 2, 1, 0);
                if (code != 0) continue;
                int width = tcp ? 24 : 12;
                int rows = Marshal.ReadInt32(table);
                if (rows < 0 || 4L + rows * width > size) throw new InvalidOperationException("Bad endpoint table");
                int owner = 0;
                for (int i = 0; i < rows; ++i) {
                    int offset = 4 + i * width;
                    int address = Marshal.ReadInt32(table, offset + (tcp ? 4 : 0));
                    int wire = Marshal.ReadInt32(table, offset + (tcp ? 8 : 4));
                    int localPort = ((wire & 255) << 8) | ((wire >> 8) & 255);
                    if (localPort != port) continue;
                    if (tcp && address != 0x0100007f) return -1;
                    if (address != 0 && address != 0x0100007f) continue;
                    if (owner != 0) return -1;
                    owner = Marshal.ReadInt32(table, offset + width - 4);
                }
                return owner;
            } finally { Marshal.FreeHGlobal(table); }
        }
        throw new Win32Exception((int)code);
    }

    public static int SdkOwner() { return Owner(17422, true); }

    public sealed class Frame {
        public long observed_tick;
        public int port, source_port, backend_pid, rgb_bytes;
        public string source, datagram;
    }

    readonly UdpClient client;
    readonly int rgbBytes;

    public PrismestraCiDdpReceiver(int port, int leds) {
        rgbBytes = leds * 3;
        client = new UdpClient(AddressFamily.InterNetwork);
        try {
            client.ExclusiveAddressUse = true;
            client.Client.Bind(new IPEndPoint(IPAddress.Loopback, port));
        } catch { client.Dispose(); throw; }
    }

    public Frame ReadFrame(int backendPid) {
        for (int i = 0; i < 128 && client.Client.Poll(0, SelectMode.SelectRead); ++i) {
            IPEndPoint source = new IPEndPoint(IPAddress.Any, 0);
            byte[] data = client.Receive(ref source);
            if (!IPAddress.Loopback.Equals(source.Address) || data.Length != 10 + rgbBytes ||
                data[0] != 0x41 || data[1] > 15 || data[2] != 1 || data[3] != 1 ||
                data[4] != 0 || data[5] != 0 || data[6] != 0 || data[7] != 0 ||
                (data[8] << 8 | data[9]) != rgbBytes) continue;
            bool nonblack = false;
            for (int j = 10; j < data.Length; ++j) nonblack |= data[j] != 0;
            if (nonblack && Owner(source.Port, false) == backendPid) {
                return new Frame { observed_tick = Stopwatch.GetTimestamp(),
                    port = ((IPEndPoint)client.Client.LocalEndPoint).Port, source_port = source.Port,
                    backend_pid = backendPid, rgb_bytes = rgbBytes, source = source.Address.ToString(),
                    datagram = BitConverter.ToString(data) };
            }
        }
        return null;
    }

    public void Dispose() { client.Dispose(); }
}
