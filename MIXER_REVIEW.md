# Mixer Code Review - Critical Issues Found

## Executive Summary

The mixer implementation has **12 critical issues**, **8 high-priority issues**, and **6 medium-priority issues**. The most severe problems involve data races, memory leaks, potential deadlocks, and arithmetic overflow in audio processing.

---

## CRITICAL Issues (Production Blockers)

### 1. **Data Race: DTMF/Gain Variables** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:772, 804-829, 1140-1145`

**Issue**: Multiple global variables accessed concurrently without synchronization:
- `g_mx_gain_in`, `g_mx_gain_dtmf` (read at line 772, 822; written at 1140-1141)
- `g_mx_dtmf_seq`, `g_mx_dtmf_idx`, `g_mx_dtmf_elapsed_ms` (read/write in mx_src_thread; written in sip_mixer_config)

**Race scenario**:
```c
// Thread 1 (mx_src_thread):
char digit = g_mx_dtmf_seq[g_mx_dtmf_idx % g_mx_dtmf_len];  // Line 805

// Thread 2 (sip_mixer_config via Rust):
g_mx_dtmf_idx = 0;  // Line 1142
g_mx_dtmf_len = new_len;  // Line 1132
```

**Impact**:
- Torn reads/writes (non-atomic doubles on 32-bit systems)
- Out-of-bounds array access if `g_mx_dtmf_len` changes mid-read
- Incorrect DTMF tone generation
- Potential crash or garbled audio

**Fix**: Protect with mutex or use atomics

---

### 2. **Division by Zero: DTMF Length** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:805, 829`

**Issue**:
```c
char digit = g_mx_dtmf_seq[g_mx_dtmf_idx % g_mx_dtmf_len];  // Line 805
g_mx_dtmf_idx = (g_mx_dtmf_idx + 1) % g_mx_dtmf_len;       // Line 829
```

If `sip_mixer_config()` is called with an empty string AND the code path at line 1132 somehow results in `g_mx_dtmf_len = 0`, this causes division by zero.

**How it can happen**:
- Current code has `g_mx_dtmf_len = n ? n : 1;` which should prevent it
- BUT if there's a race condition or future refactoring removes this check, instant crash

**Fix**: Add explicit check in mx_src_thread: `if (g_mx_dtmf_len == 0) continue;`

---

### 3. **Integer Overflow: DTMF Mixing** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:822`

**Issue**:
```c
double s = sin(g_mx_ph1) + sin(g_mx_ph2);  // s ∈ [-2, 2]
acc[i] += (int32_t)(s * (double)INT16_MAX * g_mx_gain_dtmf);
```

With `g_mx_gain_dtmf = 0.5`:
- Max value: `2 * 32767 * 0.5 = 32767` ✓
- BUT with `g_mx_gain_dtmf = 1.0`: `2 * 32767 * 1.0 = 65534`
- If input audio also adds 32767: `acc[i] = 32767 + 65534 = 98301`
- Later clamped to INT16_MAX, but the mixing math is wrong

**Also at line 772**: Input gain can cause similar overflow if mixing multiple sources.

**Impact**: Distorted audio, clipping artifacts

**Fix**: Clamp intermediate values or adjust gain calculation

---

### 4. **Memory Leak: Auplay Registration** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:1022-1025, 1100-1120`

**Issue**: `g_mx_play` registered at line 1022 but never unregistered in `sip_mixer_shutdown()`.

```c
// Init:
auplay_register(&g_mx_play, baresip_auplayl(), "b2b_mix", mx_play_alloc);

// Shutdown:
if (g_mx_src) {
    mem_deref(g_mx_src);  // Source cleaned up
    g_mx_src = NULL;
}
// g_mx_play is NEVER freed!
```

**Impact**: Memory leak on shutdown; calling init/shutdown repeatedly leaks more memory

**Fix**: Add `mem_deref(g_mx_play); g_mx_play = NULL;` in shutdown

---

### 5. **Resource Leak: Mutex Never Destroyed** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:499-507, 1100-1120`

**Issue**: `mtx_init(&g_mx_lock)` called but never `mtx_destroy()` in shutdown.

**Impact**:
- Resource leak (mutex handle)
- Calling `sip_mixer_init()` twice without restarting process leaves orphaned mutex
- On some systems (Windows), mutexes are kernel objects that consume resources

**Fix**: Add `mtx_destroy(&g_mx_lock)` in shutdown and reset `g_mx_lock_ready = false`

---

### 6. **Use-After-Free Risk: mx_play_stop** ⚠️ CRITICAL
**Location**: `c/sip_shim.c:536-540`

**Issue**:
```c
if (st->leg) {
    st->leg->play = NULL;  // Clear back-pointer
    mem_deref(st->leg);    // Free the leg
    st->leg = NULL;        // Clear pointer
}
```

**Race scenario**:
1. Thread A calls `mx_play_stop(st)`
2. Thread A sets `st->leg->play = NULL`
3. Thread B (mx_src_thread) is iterating `g_mx_legs` and accesses `leg->play`
4. Thread A calls `mem_deref(st->leg)` → destructor runs → leg freed
5. Thread B accesses freed memory → crash

**Current protection**: mx_src_thread does check `if (!leg->play)` at line 754, but the check happens AFTER acquiring the lock, and the play pointer is cleared OUTSIDE the lock.

**Fix**: Clear `leg->play` under the lock, or use RCU pattern

---

### 7. **Thread Leak: Event Handler Never Joined (Rust)** ⚠️ CRITICAL
**Location**: `src/roles/mixer.rs:36-68`

**Issue**: Same as source role - event handler thread spawned but never joined:
```rust
std::thread::spawn(move || {
    let mut sink_reported = false;
    while let Ok(ev) = rx_ev.recv() {
        // Process events forever
    }
});  // Handle dropped, thread orphaned
```

**Impact**: Zombie thread continues running after mixer role exits

**Fix**: Store handle and join on shutdown

---

### 8. **Ctrl-C Handler Registration (Rust)** ⚠️ CRITICAL
**Location**: `src/roles/mixer.rs:126-132`

**Issue**: Same pattern as source role - handler registered every time `wait_for_ctrl_c()` is called:
```rust
fn wait_for_ctrl_c() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = ctrlc::set_handler(move || {  // No Once guard!
        let _ = tx.send(());
    });
    let _ = rx.recv();
}
```

**Impact**:
- If mixer role is restarted within same process, handler conflicts
- Less severe than source (only called once) but still incorrect pattern

**Fix**: Use `Once` guard like we fixed for source

---

## HIGH Priority Issues

### 9. **Potential List Corruption on Thread Creation Failure**
**Location**: `c/sip_shim.c:668-671`

**Issue**:
```c
if (0 != thread_create_name(&st->th, "mx_play", mx_play_thread, st)) {
    mem_deref(st);  // Calls destructor
    return ENOMEM;
}
```

But `st->leg` was already added to `g_mx_legs` at line 664! When `mem_deref(st)` is called, the destructor calls `mx_play_stop()` which tries to clean up `st->leg`, but the ordering could cause issues.

**Fix**: Don't add to list until after thread creation succeeds

---

### 10. **Unsafe env::set_var (Rust)**
**Location**: `src/roles/mixer.rs:24-26, 91-93`

**Issue**: Same as other roles - unsafe in multi-threaded context.

---

### 11. **Busy-Wait Spinlock**
**Location**: `c/sip_shim.c:714-738`

**Issue**: mx_src_thread busy-waits with lock/unlock in tight loop:
```c
while (st->run && !ready) {
    mtx_lock(&g_mx_lock);
    // Check if any leg has enough data
    mtx_unlock(&g_mx_lock);
    if (ready || !ptime)
        break;
    sys_msleep(1);  // Sleep 1ms then retry
}
```

**Impact**: CPU waste, poor power efficiency

**Fix**: Use condition variable or increase sleep duration

---

### 12. **Double-Check Pattern Without Memory Barrier**
**Location**: `c/sip_shim.c:584-603`

**Issue**:
```c
if (st->leg && st->leg->buf) {  // Check outside lock
    if (g_mx_lock_ready)
        mtx_lock(&g_mx_lock);
    if (st->leg && st->leg->buf) {  // Check again inside lock
        (void)aubuf_write_auframe(...);
    }
    mtx_unlock(&g_mx_lock);
}
```

This is a TOCTOU (Time-Of-Check-Time-Of-Use) pattern. The first check isn't protected, so pointer could be freed between checks.

**Fix**: Remove outer check or use atomic pointer

---

### 13. **Possible Infinite Loop in mx_leg_remove_all**
**Location**: `c/sip_shim.c:902-918`

**Issue**:
```c
for (;;) {
    mtx_lock(&g_mx_lock);
    struct le *le = list_head(&g_mx_legs);
    if (!le) { ... break; }
    list_unlink(le);
    mtx_unlock(&g_mx_lock);  // Unlock before free
    mem_deref(leg);
}
```

If new legs are added concurrently (e.g., new incoming call), this could loop indefinitely.

**Current mitigation**: Only called during shutdown when no new calls should arrive, but not enforced.

**Fix**: Prevent new legs from being added during shutdown

---

### 14. **Metrics Not Protected by Lock**
**Location**: `c/sip_shim.c:759, 778-781, 793-798, 814, 942-975`

**Issue**: `g_mx_m` struct (metrics counters) accessed without lock in mx_src_thread but read under lock in mx_metrics_tick.

**Impact**: Torn reads of uint64 counters on 32-bit systems; incorrect metrics

**Fix**: Use atomics or ensure all access under lock

---

### 15. **Lock Not Checked After Init Failure**
**Location**: `c/sip_shim.c:499-507`

**Issue**:
```c
if (mtx_init(&g_mx_lock, mtx_plain) == thrd_success) {
    list_init(&g_mx_legs);
    g_mx_lock_ready = true;
}
// If init fails, g_mx_lock_ready stays false
// But code continues and tries to use lock anyway
```

Many places check `if (g_mx_lock_ready)` but what happens if it's false? Code proceeds without synchronization!

**Fix**: Return error if lock init fails

---

### 16. **No Bounds Check on Phase Accumulation**
**Location**: `c/sip_shim.c:818-821`

**Issue**:
```c
g_mx_ph1 += g_mx_inc1;
g_mx_ph2 += g_mx_inc2;
if (g_mx_ph1 > 2.0 * M_PI) g_mx_ph1 -= 2.0 * M_PI;
if (g_mx_ph2 > 2.0 * M_PI) g_mx_ph2 -= 2.0 * M_PI;
```

If `g_mx_inc` is very large (malformed sample rate), phases could grow unbounded between checks, leading to precision loss or overflow.

**Fix**: Use fmod or ensure inc values are validated

---

## MEDIUM Priority Issues

### 17. **Magic Numbers**
- `MX_MAX_BACKLOG_MS = 250` (line 407)
- `MX_PRELOAD_FRAMES = 6` (line 408)
- `MX_PRIME_EXTRA_FRAMES = 3` (line 409)
- `g_mx_dtmf_buf[128]` (line 395)

Should be configurable or documented.

---

### 18. **Non-Atomic Boolean Flags**
**Location**: Multiple (`st->run`, `st->primed`, `g_mx_first_in`, `g_mx_play_registered`)

These are set/read across threads without atomics. While booleans are often atomic on modern hardware, C11 standard doesn't guarantee it.

**Fix**: Use `atomic_bool` from `<stdatomic.h>`

---

### 19. **Silent Failure on Memory Allocation**
**Location**: `c/sip_shim.c:551-553, 698-708`

When `mem_alloc` fails, threads sleep and retry infinitely. No error propagation.

**Fix**: Limit retry count and call error handler

---

### 20. **Primed Flag Race**
**Location**: `c/sip_shim.c:568, 595-598, 723-724`

`st->primed` set without atomic, checked without lock in mx_src_thread (line 723).

---

### 21. **DTMF Sequence Not Validated**
**Location**: `c/sip_shim.c:1123-1147, src/roles/mixer.rs:85-90`

User can pass any string as DTMF sequence. Invalid characters silently ignored (mx_dtmf_lookup returns false), but no validation at API level.

**Fix**: Validate sequence in Rust before passing to C

---

### 22. **Inconsistent Error Handling Pattern**
Multiple places use `err |= foo()` pattern which loses specific error information.

---

## Summary Table

| Severity | Count | Examples |
|----------|-------|----------|
| Critical | 8 | Data races, div-by-zero, memory leaks, thread leaks |
| High | 8 | List corruption, spinlocks, TOCTOU, infinite loops |
| Medium | 6 | Magic numbers, atomics, validation |
| **Total** | **22** | |

---

## Recommended Actions

### Immediate (Before Production):
1. Fix data races on DTMF/gain variables (add mutex)
2. Fix memory leaks (auplay registration, mutex)
3. Fix division by zero check
4. Fix Rust thread leaks (event handler, Ctrl-C)
5. Validate DTMF mixing math to prevent overflow

### Short-term:
6. Replace busy-wait spinlock with condvar
7. Fix TOCTOU patterns with proper locking
8. Add bounds checking on audio math
9. Use atomics for boolean flags
10. Add error limits on memory allocation retries

### Medium-term:
11. Add comprehensive thread sanitizer testing
12. Add fuzzing for audio mixing logic
13. Document threading model and lock ordering
14. Add metrics for detecting races/corruption
15. Consider rewriting mixer in Rust for memory safety

---

## Testing Recommendations

1. **Thread Sanitizer**: Run with `-fsanitize=thread` to catch races
2. **Address Sanitizer**: Run with `-fsanitize=address` to catch use-after-free
3. **Valgrind**: Check for memory leaks during init/shutdown cycles
4. **Stress Test**: Rapid connect/disconnect cycles to expose races
5. **Fuzzing**: Feed random DTMF sequences and gain values
6. **Long-running**: 24-hour soak test to expose slow leaks

The mixer code requires significant hardening before production use.