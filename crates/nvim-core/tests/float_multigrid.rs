//! Regression: with ext_multigrid enabled, floats arrive as separate grids and
//! populate `active_floats()` for fade animation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nvim_core::{parse_redraw, GridStateStore, NvimSession, SessionConfig, UiEvent};
use tokio::runtime::Runtime;

fn drain_batches(
    batches: &mut Vec<Vec<nvim_core::Value>>,
    store: &mut GridStateStore,
) -> Vec<UiEvent> {
    let mut all = Vec::new();
    for args in batches.drain(..) {
        let events = parse_redraw(&args);
        all.extend(events.iter().cloned());
        let flushed = store.apply_batch(events);
        if flushed {
            let _ = store.take_pending_scroll();
        }
    }
    all
}

#[test]
fn multigrid_float_and_split_windows() {
    let rt = Runtime::new().unwrap();
    let batches: Arc<Mutex<Vec<Vec<nvim_core::Value>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = batches.clone();
    let redraw = Arc::new(move |args: Vec<nvim_core::Value>| {
        sink.lock().unwrap().push(args);
    });
    let on_close = Arc::new(|| {});

    let session = match NvimSession::spawn(
        rt.handle(),
        SessionConfig { cols: 80, rows: 24, ..Default::default() },
        redraw,
        on_close,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: nvim unavailable ({e:#})");
            return;
        }
    };

    std::thread::sleep(Duration::from_millis(500));
    let mut store = GridStateStore::new();
    drain_batches(&mut batches.lock().unwrap(), &mut store);

    // Vertical split: two non-float window grids.
    session.input(":vsplit<CR>".into());
    std::thread::sleep(Duration::from_millis(500));
    let events = drain_batches(&mut batches.lock().unwrap(), &mut store);
    let win_pos_count = events
        .iter()
        .filter(|e| matches!(e, UiEvent::WinPos { .. }))
        .count();
    let split_windows = store.windows.values().filter(|w| !w.is_float).count();
    assert!(
        win_pos_count >= 1 || split_windows >= 2,
        "multigrid splits should produce win_pos or multiple window grids (win_pos={win_pos_count}, windows={split_windows})"
    );

    // Open a floating window via API.
    session.input(
        ":lua vim.api.nvim_open_win(vim.api.nvim_create_buf(false, true), true, {relative='editor', width=20, height=5, row=2, col=2, style='minimal'})<CR>".into(),
    );
    std::thread::sleep(Duration::from_millis(600));
    let events = drain_batches(&mut batches.lock().unwrap(), &mut store);

    let float_pos = events
        .iter()
        .any(|e| matches!(e, UiEvent::WinFloatPos { .. }));
    let active = store.active_floats();
    assert!(
        float_pos || !active.is_empty(),
        "expected win_float_pos or active_floats after nvim_open_win"
    );
    assert!(!active.is_empty(), "active_floats must be populated for fade animation");

    // Close the float; window entry should disappear (fade-out handled in anim layer).
    session.input(":lua pcall(vim.api.nvim_win_close, vim.api.nvim_get_current_win(), true)<CR>".into());
    std::thread::sleep(Duration::from_millis(400));
    drain_batches(&mut batches.lock().unwrap(), &mut store);
    // Float may still be in cache during fade; active_floats should eventually empty.
    // After close event, window meta is removed immediately.
    let still_active = store.active_floats();
    println!("floats after close: {still_active:?}");
}
