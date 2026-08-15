//! Standalone CDP verification: spawn chrome, open a page, wait for one
//! real screencast frame. Run: cargo run -p webview-cdp --example cdp_probe

use std::time::{Duration, Instant};
use webview_cdp::{ChromeEngine, WebView, WebViewEvent};

fn main() {
    let dir = std::env::temp_dir().join("webview-cdp-probe");
    let _ = std::fs::remove_dir_all(&dir);
    let t0 = Instant::now();
    println!("[{:?}] spawning chrome…", t0.elapsed());
    let engine = ChromeEngine::spawn("google-chrome", 0, &dir).expect("spawn chrome");
    println!("[{:?}] engine ready, CDP port {}", t0.elapsed(), engine.port);

    println!("[{:?}] opening webview…", t0.elapsed());
    let view = engine
        .new_webview("data:text/html,<title>probe</title><h1 style=\"font-size:80px\">probe-page</h1>")
        .expect("open webview");
    println!("[{:?}] webview {} attached", t0.elapsed(), view.id());

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if Instant::now() > deadline {
            panic!("no frame within 30s");
        }
        match view.events().try_recv() {
            Ok(WebViewEvent::Frame { data, width, height }) => {
                println!(
                    "[{:?}] FRAME {}x{} ({} bytes), png magic: {}",
                    t0.elapsed(),
                    width,
                    height,
                    data.len(),
                    &data[..4] == [0x89, b'P', b'N', b'G']
                );
                std::fs::write("/tmp/probe-frame.png", &data).unwrap();
                println!("frame written to /tmp/probe-frame.png");
                break;
            }
            Ok(ev) => {
                println!("[{:?}] event: {ev:?}", t0.elapsed());
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    Box::new(view).close().unwrap();
    engine.shutdown();
    println!("done in {:?}", t0.elapsed());
}
