You are implementing 4coder/Ryan Fleury-style GUI animations for a code editor.

Goal:
Add smooth, polished editor animations inspired by Ryan Fleury’s 4coder layer:
- interpolated cursor movement
- cursor trails / ghost cursor
- cursor glow / fake bloom
- squash/stretch cursor movement
- smooth scrolling
- flash/fading highlights
- optional “power mode” typing particles and screen shake

Do not implement these as blocking animations. The editor must remain responsive. Animations must be frame-time based and should request redraws only while something is still animating.

Architecture requirements:
1. Add persistent animation state separate from editor model state.
2. Keep “logical state” and “rendered animated state” separate.
   - Logical cursor position is the real text cursor.
   - Render cursor position is an interpolated rectangle moving toward the logical cursor rectangle.
3. Every frame:
   - compute `dt`
   - update animation state
   - render from animation state
   - if any animation is still active, request another frame
4. Use frame-delta-based animation, not fixed sleeps or timers inside logic.
5. All animation features must be configurable and easy to disable.

Core config:
Add a config struct like:

```cpp
struct AnimationConfig {
    bool enable_cursor_animation = true;
    bool enable_cursor_trail = true;
    bool enable_cursor_glow = true;
    bool enable_cursor_squash_stretch = true;
    bool enable_smooth_scroll = true;
    bool enable_flashes = true;
    bool enable_power_mode = false;

    float cursor_speed = 30.0f;
    float cursor_snap_epsilon = 0.5f;
    float cursor_stretch_strength = 1.0f;
    float cursor_max_stretch = 3.0f;

    int cursor_glow_layers = 20;
    float cursor_glow_radius = 20.0f;
    float cursor_glow_alpha = 0.20f;

    float scroll_smooth_time = 0.10f;
    float flash_duration = 0.35f;

    int particles_per_key = 30;
    float particle_lifetime = 0.6f;
    float screen_shake_decay = 10.0f;
};
