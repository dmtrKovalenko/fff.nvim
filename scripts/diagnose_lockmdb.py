#!/usr/bin/env python3
"""
Diagnose LMDB lock.mdb holders and waiters.

Usage: ./diagnose_lockmdb.py [path_to_lock.mdb]
Default: /home/neogoose/.cache/nvim/fff_nvim/lock.mdb

Shows:
1. Every process that has the lock.mdb inode in its mmap/fd table (potential holders)
2. Every process currently blocked on a futex at an address in the lock.mdb mmap
   (actually waiting on the reader/writer mutex)
3. The mutex state (__lock / __owner / __kind) for both reader and writer mutex
"""

import os
import struct
import sys
from pathlib import Path

LOCK_PATH = sys.argv[1] if len(sys.argv) > 1 else "/home/neogoose/.cache/nvim/fff_nvim/lock.mdb"

RMUTEX_OFFSET = 24   # MDB_txbody.mtb_rmutex
WMUTEX_OFFSET = 64   # MDB_txninfo.mt2.mt2_wmutex
MUTEX_SIZE = 40      # sizeof(pthread_mutex_t) on glibc x86_64


def mutex_state(data, off):
    """Decode pthread_mutex_t at offset `off`."""
    if len(data) < off + 20:
        return None
    return {
        "lock":   struct.unpack("i", data[off:off + 4])[0],
        "count":  struct.unpack("I", data[off + 4:off + 8])[0],
        "owner":  struct.unpack("i", data[off + 8:off + 12])[0],
        "nusers": struct.unpack("I", data[off + 12:off + 16])[0],
        "kind":   struct.unpack("i", data[off + 16:off + 20])[0],
    }


def fmt_mutex(m):
    if m is None:
        return "(too small)"
    robust = "ROBUST" if m["kind"] & 0x10 else "non-robust"
    pshared = "PSHARED" if m["kind"] & 0x80 else "private"
    state = {0: "unlocked", 1: "locked", 2: "locked+waiters"}.get(m["lock"], f"lock={m['lock']}")
    return (f"state={state} owner_tid={m['owner']} nusers={m['nusers']} "
            f"kind=0x{m['kind']:x} ({robust}, {pshared})")


def get_comm(pid):
    try:
        return Path(f"/proc/{pid}/comm").read_text().strip()
    except Exception:
        return "?"


def get_cmdline(pid):
    try:
        return Path(f"/proc/{pid}/cmdline").read_text().replace("\0", " ").strip()[:100]
    except Exception:
        return "?"


def get_wchan(pid_or_tid, path):
    try:
        return Path(f"{path}/wchan").read_text().strip()
    except Exception:
        return "?"


def get_syscall(tid_path):
    """Return (syscall_nr, arg0, arg1, ..., arg5) or None."""
    try:
        parts = Path(f"{tid_path}/syscall").read_text().split()
        if parts and parts[0] != "running":
            return [int(p, 0) for p in parts[:7]]
    except Exception:
        pass
    return None


def get_mmap_regions(pid, target_inode):
    """Return list of (start, end) virtual addresses where `target_inode` is mapped."""
    regions = []
    try:
        for line in Path(f"/proc/{pid}/maps").read_text().splitlines():
            parts = line.split()
            if len(parts) < 5:
                continue
            # inode is field index 4
            try:
                inode = int(parts[4])
            except ValueError:
                continue
            if inode != target_inode:
                continue
            addr_range = parts[0]
            try:
                start_s, end_s = addr_range.split("-")
                regions.append((int(start_s, 16), int(end_s, 16), line))
            except Exception:
                continue
    except Exception:
        pass
    return regions


def main():
    path = Path(LOCK_PATH)
    if not path.exists():
        print(f"ERROR: {path} does not exist")
        return 1

    st = path.stat()
    target_inode = st.st_ino
    size = st.st_size
    print(f"=== Target: {path} ===")
    print(f"   inode={target_inode}  size={size}\n")

    # Read mutex state from the on-disk file
    data = path.read_bytes()
    r_mutex = mutex_state(data, RMUTEX_OFFSET)
    w_mutex = mutex_state(data, WMUTEX_OFFSET)
    print("=== Mutex state on disk ===")
    print(f"  READER mutex (offset {RMUTEX_OFFSET}): {fmt_mutex(r_mutex)}")
    print(f"  WRITER mutex (offset {WMUTEX_OFFSET}): {fmt_mutex(w_mutex)}")
    print()

    # Scan all processes for mmap hits on this inode.
    holders = []  # list of (pid, start, end, deleted)
    for pid_dir in Path("/proc").iterdir():
        if not pid_dir.name.isdigit():
            continue
        pid = int(pid_dir.name)
        regions = get_mmap_regions(pid, target_inode)
        if not regions:
            # Also check fd table — inode may be open-but-not-mmap'd
            try:
                for fd in (pid_dir / "fd").iterdir():
                    try:
                        tgt = os.readlink(fd)
                    except OSError:
                        continue
                    if "lock.mdb" in tgt and f"[{target_inode}]" in os.stat(fd).st_dev.__str__():
                        # crude — we already caught mmap above, skip
                        pass
            except OSError:
                pass
            continue
        for (start, end, line) in regions:
            deleted = "(deleted)" in line
            holders.append((pid, start, end, deleted))

    print("=== Processes with lock.mdb mmap'd ===")
    if not holders:
        print("  (none — no live process holds this inode via mmap)")
    for (pid, start, end, deleted) in holders:
        marker = " (DELETED inode — stale mapping from previous file)" if deleted else ""
        print(f"  PID {pid} ({get_comm(pid)})  mmap @ 0x{start:x}-0x{end:x}{marker}")
        print(f"    cmdline: {get_cmdline(pid)}")

    print()

    # For each holder mmap, compute the futex addresses of each mutex,
    # then scan all threads in all processes for futex syscalls targeting those addresses.
    futex_addrs = {}  # address -> description
    for (pid, start, _, deleted) in holders:
        futex_addrs[start + RMUTEX_OFFSET] = (pid, "READER mutex", deleted)
        futex_addrs[start + WMUTEX_OFFSET] = (pid, "WRITER mutex", deleted)

    print("=== Threads currently blocked on mutex futex ===")
    found_any = False
    for pid_dir in Path("/proc").iterdir():
        if not pid_dir.name.isdigit():
            continue
        pid = int(pid_dir.name)
        task_dir = pid_dir / "task"
        if not task_dir.exists():
            continue
        try:
            tids = [t.name for t in task_dir.iterdir() if t.name.isdigit()]
        except OSError:
            continue
        for tid in tids:
            tid_path = task_dir / tid
            sc = get_syscall(tid_path)
            if sc is None:
                continue
            # syscall 202 = futex on x86_64
            if sc[0] != 202:
                continue
            futex_addr = sc[1]
            # Match: the blocked thread must have this address mapped into its own address space.
            # Since mmap addresses differ per process, compare only against THIS process's own holders.
            my_regions = get_mmap_regions(pid, target_inode)
            for (start, end, line) in my_regions:
                deleted = "(deleted)" in line
                r_addr = start + RMUTEX_OFFSET
                w_addr = start + WMUTEX_OFFSET
                if futex_addr == r_addr:
                    wchan = get_wchan(tid, tid_path)
                    dmark = " (DELETED)" if deleted else ""
                    print(f"  PID {pid} TID {tid} ({get_comm(pid)}) wchan={wchan}: "
                          f"blocked on READER mutex @ 0x{futex_addr:x}{dmark}")
                    found_any = True
                elif futex_addr == w_addr:
                    wchan = get_wchan(tid, tid_path)
                    dmark = " (DELETED)" if deleted else ""
                    print(f"  PID {pid} TID {tid} ({get_comm(pid)}) wchan={wchan}: "
                          f"blocked on WRITER mutex @ 0x{futex_addr:x}{dmark}")
                    found_any = True
    if not found_any:
        print("  (none)")

    print()
    print("=== /proc/locks fcntl/flock state ===")
    try:
        for line in Path("/proc/locks").read_text().splitlines():
            if str(target_inode) in line:
                # format: N: TYPE ACCESS KIND PID MAJ:MIN:INODE START END
                parts = line.split()
                if len(parts) >= 7 and parts[6].endswith(f":{target_inode}"):
                    pid = parts[4]
                    print(f"  {line}")
                    print(f"     -> PID {pid} ({get_comm(pid)})")
    except Exception as e:
        print(f"  error reading /proc/locks: {e}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
