"""Host and process identity for Linux and macOS; no persistent host writes."""
import ctypes
import os
from pathlib import Path
import re
import subprocess
import sys


def output(argv):
    return subprocess.check_output(argv, text=True, timeout=5).strip()


def machine_id():
    if sys.platform == 'linux':
        return Path('/etc/machine-id').read_text().strip()
    if sys.platform == 'darwin':
        value = output(['/usr/sbin/ioreg', '-rd1', '-c', 'IOPlatformExpertDevice'])
        match = re.search(r'"IOPlatformUUID"\s*=\s*"([^"\n]+)"', value)
        if match:
            return match[1]
    raise ValueError('Не удалось получить устойчивую identity машины')


class BSDInfo(ctypes.Structure):
    # Public XNU proc_bsdinfo ABI; MAXCOMLEN=16, PROC_PIDTBSDINFO=3.
    _fields_ = [(name, ctypes.c_uint32) for name in
                ('flags', 'status', 'xstatus', 'pid', 'ppid', 'uid', 'gid',
                 'ruid', 'rgid', 'svuid', 'svgid', 'reserved')] + [
        ('comm', ctypes.c_char * 16), ('name', ctypes.c_char * 32),
        *[(name, ctypes.c_uint32) for name in ('nfiles', 'pgid', 'pjobc', 'tdev', 'tpgid')],
        ('nice', ctypes.c_int32), ('start_sec', ctypes.c_uint64), ('start_usec', ctypes.c_uint64)]


def mac_info(pid):
    lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
    lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                ctypes.c_void_p, ctypes.c_int]
    lib.proc_pidinfo.restype = ctypes.c_int
    info = BSDInfo()
    size = ctypes.sizeof(info)
    if lib.proc_pidinfo(pid, 3, 0, ctypes.byref(info), size) != size:
        return None
    return info


def process_identity(pid):
    try:
        if sys.platform == 'linux':
            stat = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
            return dict(boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                        start_ticks=stat[19])
        if sys.platform == 'darwin':
            info = mac_info(pid)
            if info is None:
                return None
            return dict(boot_id=output(['/usr/sbin/sysctl', '-n', 'kern.boottime']),
                        start_sec=info.start_sec, start_usec=info.start_usec)
    except (OSError, subprocess.SubprocessError):
        return None
    raise ValueError('Поддерживаются Linux и macOS')


def process_running(pid):
    try:
        if sys.platform == 'linux':
            return Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z'
        if sys.platform == 'darwin':
            info = mac_info(pid)
            return info is not None and info.status != 5  # SZOMB
    except OSError:
        return False
    raise ValueError('Поддерживаются Linux и macOS')
