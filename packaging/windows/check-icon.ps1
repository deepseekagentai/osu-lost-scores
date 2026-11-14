param([Parameter(Mandatory = $true)][string]$Exe)

$ErrorActionPreference = "Stop"

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class IconProbe
{
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr LoadLibraryEx(string file, IntPtr handle, uint flags);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern IntPtr LockResource(IntPtr resource);

    public static int Images(string file)
    {
        IntPtr module = LoadLibraryEx(file, IntPtr.Zero, 0x2);
        if (module == IntPtr.Zero) return -1;
        IntPtr group = FindResource(module, (IntPtr)1, (IntPtr)14);
        if (group == IntPtr.Zero) return 0;
        IntPtr data = LockResource(LoadResource(module, group));
        return Marshal.ReadInt16(data, 4);
    }
}
"@

$count = [IconProbe]::Images((Resolve-Path $Exe).Path)
Write-Host "icon images in ${Exe}: $count"
if ($count -lt 7) {
    Write-Error "the executable does not carry the lemon icon"
    exit 1
}
