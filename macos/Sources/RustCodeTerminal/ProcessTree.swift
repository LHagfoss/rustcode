import Darwin
import Foundation

/// Ends a terminal's shell and the processes it started.
///
/// Closing a terminal window hangs up its PTY: the kernel sends SIGHUP and
/// shells pass it on to their jobs. This does the same explicitly, also to
/// stopped jobs and to everything below the shell found before it exits (a
/// shell's children are reparented once it is gone), then SIGKILLs whatever
/// is left once the grace period ends. Only a session the user closes, or
/// the app quitting, ends a shell this way.
struct ShellTermination: Sendable {
    let shell: pid_t
    let foregroundGroup: pid_t
    let descendants: [pid_t]

    /// Read before the PTY closes: the foreground job and the process tree.
    init(shell: pid_t, ptyDescriptor: Int32) {
        self.shell = shell
        self.foregroundGroup = ptyDescriptor >= 0 ? tcgetpgrp(ptyDescriptor) : -1
        self.descendants = ProcessTree.descendants(of: shell)
    }

    /// SIGHUP, then SIGCONT so stopped jobs see it.
    func hangUp() {
        if foregroundGroup > 0 { killpg(foregroundGroup, SIGHUP) }
        killpg(shell, SIGHUP)
        kill(shell, SIGHUP)
        for pid in descendants { kill(pid, SIGHUP) }
        if foregroundGroup > 0 { killpg(foregroundGroup, SIGCONT) }
        for pid in descendants { kill(pid, SIGCONT) }
    }

    /// Waits for the shell (our child, so it is reaped here) and its tree
    /// until `deadline`, then SIGKILLs what is left. True when all ended in
    /// time.
    @discardableResult
    func finish(by deadline: Date) -> Bool {
        var reaped = false
        while Date() < deadline {
            if !reaped { reaped = ProcessTree.reap(shell) }
            if reaped && !descendants.contains(where: ProcessTree.isAlive) { return true }
            Thread.sleep(forTimeInterval: 0.02)
        }
        if foregroundGroup > 0 { killpg(foregroundGroup, SIGKILL) }
        for pid in descendants where ProcessTree.isAlive(pid) { kill(pid, SIGKILL) }
        if !reaped {
            kill(shell, SIGKILL)
            var status: Int32 = 0
            _ = waitpid(shell, &status, 0)
        }
        return false
    }
}

enum ProcessTree {
    /// Every process below `pid`, children first.
    static func descendants(of pid: pid_t, limit: Int = 4096) -> [pid_t] {
        var result: [pid_t] = []
        var queue = [pid]
        var seen: Set<pid_t> = [pid]
        while !queue.isEmpty, result.count < limit {
            let parent = queue.removeFirst()
            for child in children(of: parent) where seen.insert(child).inserted {
                result.append(child)
                queue.append(child)
            }
        }
        return result
    }

    static func children(of pid: pid_t) -> [pid_t] {
        let needed = proc_listpids(UInt32(PROC_PPID_ONLY), UInt32(pid), nil, 0)
        guard needed > 0 else { return [] }
        let stride = MemoryLayout<pid_t>.stride
        var pids = [pid_t](repeating: 0, count: Int(needed) / stride + 32)
        let bytes = pids.withUnsafeMutableBytes { buffer in
            proc_listpids(UInt32(PROC_PPID_ONLY), UInt32(pid), buffer.baseAddress, Int32(buffer.count))
        }
        guard bytes > 0 else { return [] }
        return pids.prefix(Int(bytes) / stride).filter { $0 > 0 && $0 != pid }
    }

    /// The process exists and is not a zombie waiting to be reaped.
    static func isAlive(_ pid: pid_t) -> Bool {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else {
            return kill(pid, 0) == 0
        }
        return info.pbi_status != UInt32(SZOMB)
    }

    /// Collects our child `pid` if it has exited. True once it is gone.
    static func reap(_ pid: pid_t) -> Bool {
        var status: Int32 = 0
        let result = waitpid(pid, &status, WNOHANG)
        return result == pid || (result == -1 && errno == ECHILD)
    }
}
