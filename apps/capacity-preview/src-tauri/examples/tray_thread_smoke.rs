//! Native regression smoke test; no windows, account reads, or network requests.
//! Run on a logged-in desktop with:
//! cargo run --offline --locked -p capacity-desktop --example tray_thread_smoke

use capacity_desktop_lib::with_tray_on_main_thread;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use tauri::{menu::Menu, tray::TrayIconBuilder};

const TRAY_ID: &str = "horizon-thread-smoke";
const WORKERS: usize = 8;
const UPDATES: usize = 250;

fn main() {
    // A failed UI queue or worker must fail this isolated test, not leave an
    // unattended menu-bar test process running indefinitely.
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(30));
        eprintln!("Native tray smoke timed out");
        std::process::exit(1);
    });
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "dev.quotahorizon.tray-thread-smoke".to_owned();
    let app = tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            TrayIconBuilder::with_id(TRAY_ID)
                .title("Horizon TEST")
                .tooltip("Temporary synthetic tray-thread regression test")
                .build(app)?;

            let owner = thread::current().id();
            let app = app.handle().clone();
            thread::spawn(move || {
                let completed = Arc::new(AtomicUsize::new(0));
                let failed = Arc::new(AtomicUsize::new(0));
                let mut workers = Vec::new();
                for worker in 0..WORKERS {
                    let app = app.clone();
                    let completed = completed.clone();
                    let failed = failed.clone();
                    workers.push(thread::spawn(move || {
                        for sequence in 0..UPDATES {
                            // Exercise the desktop's menu hand-off too. Menu is
                            // Arc-backed in Tauri and safely created off-thread;
                            // TrayIcon itself must not cross this boundary.
                            let menu = (sequence % 25 == 0)
                                .then(|| Menu::new(&app).expect("create synthetic menu"));
                            let completed = completed.clone();
                            let failed = failed.clone();
                            with_tray_on_main_thread(&app, TRAY_ID, move |tray| {
                                assert_eq!(thread::current().id(), owner);
                                let title = format!("Horizon TEST {worker}:{sequence}");
                                let mut ok = tray.set_title(Some(title)).is_ok();
                                ok &= tray.set_tooltip(Some("Synthetic quota update")).is_ok();
                                if let Some(menu) = menu {
                                    ok &= tray.set_menu(Some(menu)).is_ok();
                                }
                                if !ok {
                                    failed.fetch_add(1, Ordering::SeqCst);
                                }
                                completed.fetch_add(1, Ordering::SeqCst);
                            })
                            .expect("dispatch synthetic tray update");
                        }
                    }));
                }
                for worker in workers {
                    worker.join().expect("tray update worker");
                }
                // All preceding jobs must run before this final tray lookup.
                let exit_app = app.clone();
                app.run_on_main_thread(move || {
                    assert_eq!(thread::current().id(), owner);
                    let retained = exit_app.tray_by_id(TRAY_ID).is_some();
                    let count = completed.load(Ordering::SeqCst);
                    let errors = failed.load(Ordering::SeqCst);
                    println!(
                        "Native tray smoke: {count} updates, {errors} errors, {WORKERS} workers, tray retained: {retained}"
                    );
                    exit_app.exit(i32::from(
                        count != WORKERS * UPDATES || errors != 0 || !retained,
                    ));
                })
                .expect("dispatch final tray check");
            });
            Ok(())
        })
        .build(context)
        .expect("build isolated tray test");
    app.run(|_, _| {});
}
