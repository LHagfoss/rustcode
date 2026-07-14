# Scrolling Optimization Changes

## Problem
The app had several scrolling issues:
1. **Laggy scrolling** - 50ms poll interval (20Hz) caused choppy scroll
2. **Arrow key conflict** - 50ms "wheel detection window" prevented normal history navigation
3. **No scroll coalescing** - rapid scroll events caused state thrashing
4. **Missing bounds check** - `scroll_up` didn't validate against 0

## Changes Made

### 1. Increased Poll Rate (main.rs)
- **Before**: 50ms poll interval (20Hz)
- **After**: 16ms poll interval (60Hz)
- **Impact**: 3x smoother scrolling, more responsive input

### 2. Removed Arrow Key Scroll Detection (main.rs)
- **Before**: 50ms window where rapid arrow keys were treated as scroll
- **After**: Arrow keys always navigate history
- **Reasoning**: The detection was unreliable and conflicted with normal usage
- **Solution**: Use mouse wheel or Page Up/Down for scrolling

### 3. Added Scroll Coalescing (main.rs)
- **Before**: Each scroll event processed immediately
- **After**: Rapid scroll events (within 16ms) are batched
- **Impact**: Smoother scroll, less state thrashing during fast scrolling

### 4. Fixed Bounds Check (state.rs)
- **Before**: `scroll_up` used `saturating_sub` without lower bound
- **After**: Added `.max(0)` to prevent negative scroll positions

### 5. Removed Dead Code (main.rs)
- Removed `PendingArrowKey` enum
- Removed `flush_pending_history_arrow` function
- Removed `pending_arrow_key` variable and related logic

## Testing
Build succeeds with no errors (only unrelated warning in notifications.rs).

## Next Steps (if needed)
- Add scroll momentum/inertia for even smoother experience
- Consider using `event::EventStream` for truly async event handling
- Add configurable scroll speed
