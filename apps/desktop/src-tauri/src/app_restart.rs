//! Restart after cleanup, while the AppKit loop is still servicing UI work.
use tauri::{AppHandle, Manager, Runtime};

pub(crate) fn restart<R: Runtime>(
    app: AppHandle<R>,
    cleanup: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let executable = tauri::process::current_binary(&app.env())
        .map_err(|_| "Unable to locate the running application".to_owned())?;
    if !executable.is_file() {
        return Err("The application executable is no longer available".to_owned());
    }
    std::thread::Builder::new()
        .name("horizon-restart".into())
        .spawn(move || {
            cleanup();
            let restart_app = app.clone();
            // Tauri's main-thread restart is direct; request_restart instead
            // depends on another Exit event. All product cleanup is complete,
            // and the single-instance socket must be released before spawning.
            if let Err(error) = app.run_on_main_thread(move || {
                tauri_plugin_single_instance::destroy(&restart_app);
                restart_app.restart();
            }) {
                eprintln!("Unable to dispatch application restart: {error}");
            }
        })
        .map(|_| ())
        .map_err(|_| "Unable to start the application restart worker".to_owned())
}
