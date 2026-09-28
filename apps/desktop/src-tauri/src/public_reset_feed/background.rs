//! Collection belongs to the native app, not the visibility of a webview.
use super::*;
use std::sync::mpsc;

struct BackgroundCollection(Mutex<Option<mpsc::Sender<()>>>);

// Check the wall clock at least once a minute, including after wake. The shared
// collector's persisted 15-minute gate prevents duplicate network requests.
const WAKE_CHECK: Duration = Duration::from_secs(60);

fn run_until_stopped(receiver: mpsc::Receiver<()>, interval: Duration, mut collect: impl FnMut()) {
    loop {
        if receiver.try_recv() != Err(mpsc::TryRecvError::Empty) {
            break;
        }
        collect();
        if receiver.recv_timeout(interval) != Err(mpsc::RecvTimeoutError::Timeout) {
            break;
        }
    }
}

pub(crate) fn setup(app: &tauri::AppHandle) -> Result<(), String> {
    let path = data_path(app)?;
    let (stop, receiver) = mpsc::channel();
    if !app.manage(BackgroundCollection(Mutex::new(Some(stop)))) {
        return Err("Public-source collection is already running".into());
    }
    let app = app.clone();
    std::thread::Builder::new()
        .name("horizon-public-reset-refresh".into())
        .spawn(move || {
            run_until_stopped(receiver, WAKE_CHECK, || {
                match refresh_at_path(&path, false) {
                    Ok((_, true)) => {
                        let _ = app.emit("public-reset-timeline-changed", ());
                    }
                    Ok((_, false)) => {}
                    // A local-store failure must not stop the worker forever.
                    Err(_) => {
                        let _ = app.emit("public-reset-timeline-unavailable", ());
                    }
                }
            });
        })
        .map_err(|_| "Unable to start public-source background collection".to_owned())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_immediately_without_a_window_and_stops_after_shutdown() {
        let (stop, receiver) = mpsc::channel();
        let mut calls = 0;
        run_until_stopped(receiver, Duration::from_millis(1), || {
            calls += 1;
            if calls == 2 {
                stop.send(()).unwrap();
            }
        });
        assert_eq!(calls, 2);
    }

    #[test]
    fn shutdown_before_start_or_disconnection_performs_no_network_work() {
        for disconnected in [true, false] {
            let (stop, receiver) = mpsc::channel();
            if !disconnected {
                stop.send(()).unwrap();
            }
            drop(stop);
            run_until_stopped(receiver, WAKE_CHECK, || panic!("must not collect"));
        }
    }
}

pub(crate) fn shutdown<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(state) = app.try_state::<BackgroundCollection>() {
        if let Ok(mut stop) = state.0.lock() {
            if let Some(stop) = stop.take() {
                let _ = stop.send(());
            }
        }
    }
}
