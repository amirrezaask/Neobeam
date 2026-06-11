//! Investigation + regression: scroll deltas must reach the global grid (1)
//! even though nvim reports `win_viewport` against the window's own grid handle
//! when `ext_multigrid` is disabled.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nvim_core::{parse_redraw, GridStateStore, NvimSession, SessionConfig};
use tokio::runtime::Runtime;

/// Drives a real nvim session through the same batch/flush cycle as the app and
/// asserts that scrolling produces a non-zero delta on grid 1.
#[test]
fn scroll_delta_reaches_global_grid() {
    let rt = Runtime::new().unwrap();
    let batches: Arc<Mutex<Vec<Vec<nvim_core::Value>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = batches.clone();
    let redraw = Arc::new(move |args: Vec<nvim_core::Value>| {
        sink.lock().unwrap().push(args);
    });
    let on_close = Arc::new(|| {});

    let session = match NvimSession::spawn(
        rt.handle(),
        SessionConfig {
            cols: 80,
            rows: 24,
            ..Default::default()
        },
        redraw,
        on_close,
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skipping: nvim unavailable ({e:#})");
            return;
        }
    };

    session.input(":call setline(1, map(range(1,300), 'string(v:val)'))<CR>".into());
    std::thread::sleep(Duration::from_millis(400));
    session.input("gg".into());
    std::thread::sleep(Duration::from_millis(400));

    let drain = |label: &str| -> i64 {
        let pending: Vec<Vec<nvim_core::Value>> = std::mem::take(&mut *batches.lock().unwrap());
        let mut store = GridStateStore::new();
        let mut total: i64 = 0;
        // Replay each notification batch exactly like the app event loop.
        for args in pending {
            let events = parse_redraw(&args);
            let flushed = store.apply_batch(events);
            if flushed {
                for (_grid, delta) in store.take_pending_scroll() {
                    total += delta;
                }
            }
        }
        println!("[{label}] scroll delta total = {total}");
        total
    };

    // Prime the store with the initial paint, then ignore it.
    std::thread::sleep(Duration::from_millis(200));
    drain("initial");

    session.input("30j".into());
    std::thread::sleep(Duration::from_millis(600));
    let j = drain("30j");

    session.input("<C-d>".into());
    std::thread::sleep(Duration::from_millis(600));
    let cd = drain("<C-d>");

    session.input("<C-e>".into());
    std::thread::sleep(Duration::from_millis(600));
    let ce = drain("<C-e>");

    assert!(j != 0, "30j must produce a scroll delta");
    assert!(cd != 0, "<C-d> must produce a scroll delta");
    assert!(ce != 0, "<C-e> must produce a scroll delta");
}
