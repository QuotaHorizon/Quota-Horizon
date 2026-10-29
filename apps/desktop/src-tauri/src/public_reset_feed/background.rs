//! Collection belongs to the native app, not the visibility of a webview.
use super::*;
use std::sync::mpsc;

struct BackgroundCollection(Mutex<Option<Vec<mpsc::Sender<()>>>>);

// Check the wall clock at least once a minute, including after wake. The shared
// collector's persisted minute gate prevents duplicate network requests.
const WAKE_CHECK: Duration = Duration::from_secs(60);
const OUTPUT_CHECK: Duration = Duration::from_secs(3);

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
    let (output_stop, output_receiver) = mpsc::channel();
    if !app.manage(BackgroundCollection(Mutex::new(Some(vec![
        stop.clone(),
        output_stop,
    ])))) {
        return Err("Public-source collection is already running".into());
    }
    let output_app = app.clone();
    let output_directory = path.parent().unwrap_or(&path).to_owned();
    let app = app.clone();
    std::thread::Builder::new()
        .name("horizon-public-reset-refresh".into())
        .spawn(move || {
            let mut quantitative_signature = String::new();
            let mut supervisor = quantitative::runtime::Supervisor::bundled(
                app.path().resource_dir().ok(),
                path.parent().unwrap_or(&path),
            );
            run_until_stopped(receiver, WAKE_CHECK, || {
                if let Some(directory) = path.parent() {
                    supervisor.tick(directory, Utc::now());
                }
                match refresh_at_path(&path, false) {
                    Ok((timeline, collected)) => {
                        let signature = quantitative::signature(&timeline.quantitative);
                        if collected || signature != quantitative_signature {
                            let _ = app.emit("public-reset-timeline-changed", ());
                        }
                        quantitative_signature = signature;
                    }
                    // A local-store failure must not stop the worker forever.
                    Err(_) => {
                        let _ = app.emit("public-reset-timeline-unavailable", ());
                    }
                }
            });
        })
        .map_err(|_| "Unable to start public-source background collection".to_owned())?;
    std::thread::Builder::new()
        .name("horizon-reset-output-watch".into())
        .spawn(move || {
            let mut watcher = quantitative::OutputWatcher::new(&output_directory);
            run_until_stopped(output_receiver, OUTPUT_CHECK, || {
                if watcher.changed() {
                    let _ = output_app.emit("public-reset-timeline-changed", ());
                }
            });
        })
        .map_err(|_| {
            let _ = stop.send(());
            "Unable to start reset-output background observation".to_owned()
        })?;
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

    #[test]
    fn export_watcher_tracks_creation_deletion_and_connection_changes() {
        let directory = tempfile::tempdir().unwrap();
        let default_report = directory.path().join("reset-observer/report.json");
        fs::create_dir_all(default_report.parent().unwrap()).unwrap();
        let mut watcher = quantitative::OutputWatcher::new(directory.path());
        assert!(!watcher.changed());
        // The watcher does not parse documents; consumer validation remains
        // responsible for retaining the last good report during partial writes.
        fs::write(&default_report, "partial").unwrap();
        assert!(watcher.changed());
        assert!(!watcher.changed());
        fs::remove_file(&default_report).unwrap();
        assert!(watcher.changed());
        assert!(!watcher.changed());

        let connected = tempfile::tempdir().unwrap();
        let report = connected.path().join("report.json");
        let forecast = connected.path().join("consumer.json");
        let config = directory.path().join("reset-radar-connection.json");
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "forecastPath": forecast, "evaluationPath": report,
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(watcher.changed());
        fs::write(&default_report, "old connection").unwrap();
        assert!(!watcher.changed());
        fs::write(&report, "new report").unwrap();
        assert!(watcher.changed());
        fs::write(&forecast, "new forecast").unwrap();
        assert!(watcher.changed());
        assert!(!watcher.changed());

        fs::write(&config, "invalid").unwrap();
        assert!(watcher.changed());
        assert!(!watcher.changed());
        fs::write(&report, "inactive report changed").unwrap();
        assert!(!watcher.changed());
        fs::remove_file(&config).unwrap();
        assert!(watcher.changed());
        fs::write(&default_report, "default restored").unwrap();
        assert!(watcher.changed());
    }

    #[test]
    fn published_report_notifies_while_collector_is_blocked() {
        let directory = tempfile::tempdir().unwrap();
        let report = directory.path().join("reset-observer/report.json");
        fs::create_dir_all(report.parent().unwrap()).unwrap();
        fs::write(&report, "initial").unwrap();
        let mut watcher = quantitative::OutputWatcher::new(directory.path());
        let (collection_stop, collection_receiver) = mpsc::channel();
        let (watch_stop, watch_receiver) = mpsc::channel();
        let (entered, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (notify, changed) = mpsc::channel();
        let completed = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            let completion = &completed;
            scope.spawn(move || {
                run_until_stopped(collection_receiver, WAKE_CHECK, || {
                    entered.send(()).unwrap();
                    blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                    completion.store(true, std::sync::atomic::Ordering::SeqCst);
                });
            });
            scope.spawn(move || {
                run_until_stopped(watch_receiver, Duration::from_millis(1), || {
                    if watcher.changed() {
                        notify.send(()).unwrap();
                    }
                });
            });
            started.recv_timeout(Duration::from_secs(1)).unwrap();
            fs::write(&report, "published while HTTP is blocked").unwrap();
            let notification = changed.recv_timeout(Duration::from_secs(1));
            let still_blocked = !completed.load(std::sync::atomic::Ordering::SeqCst);
            watch_stop.send(()).unwrap();
            collection_stop.send(()).unwrap();
            release.send(()).unwrap();
            assert!(notification.is_ok());
            assert!(still_blocked);
        });
    }
}

pub(crate) fn shutdown<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(state) = app.try_state::<BackgroundCollection>() {
        if let Ok(mut stop) = state.0.lock() {
            if let Some(workers) = stop.take() {
                for stop in workers {
                    let _ = stop.send(());
                }
            }
        }
    }
}
