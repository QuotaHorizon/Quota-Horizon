use tauri::{AppHandle, Runtime, tray::TrayIcon};

/// Keep lookup, use, and destruction of a native tray handle on the UI thread.
///
/// Tauri 2.11's setters dispatch to the main thread, but `tray_by_id`, cloning,
/// and dropping its Rc-backed tray handle do not. Moving an already acquired
/// handle into `run_on_main_thread` is therefore too late. Only capture owned
/// presentation data in `update`; never retain or send a cloned tray handle.
pub fn with_tray_on_main_thread<R: Runtime>(
    app: &AppHandle<R>,
    tray_id: &'static str,
    update: impl FnOnce(&TrayIcon<R>) + Send + 'static,
) -> tauri::Result<()> {
    let lookup_app = app.clone();
    dispatch_scoped_resource(
        |task| app.run_on_main_thread(task),
        move || lookup_app.tray_by_id(tray_id),
        update,
    )
}

// Resource deliberately has no Send bound: it must only exist on the thread
// executing the dispatched task, including when the update finishes.
fn dispatch_scoped_resource<Resource, Error>(
    dispatch: impl FnOnce(Box<dyn FnOnce() + Send>) -> Result<(), Error>,
    acquire: impl FnOnce() -> Option<Resource> + Send + 'static,
    update: impl FnOnce(&Resource) + Send + 'static,
) -> Result<(), Error> {
    dispatch(Box::new(move || {
        if let Some(resource) = acquire() {
            update(&resource);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::dispatch_scoped_resource;
    use std::{
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread::{self, ThreadId},
    };

    struct ThreadBoundResource {
        owner: ThreadId,
        drops: Arc<AtomicUsize>,
        _not_send: Rc<()>,
    }

    impl Drop for ThreadBoundResource {
        fn drop(&mut self) {
            assert_eq!(thread::current().id(), self.owner);
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn concurrent_workers_acquire_use_and_drop_only_on_owner_thread() {
        const WORKERS: usize = 8;
        const UPDATES: usize = 100;
        let owner = thread::current().id();
        let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let acquired = Arc::new(AtomicUsize::new(0));
        let updated = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let sender = sender.clone();
            let acquired = acquired.clone();
            let updated = updated.clone();
            let dropped = dropped.clone();
            workers.push(thread::spawn(move || {
                for _ in 0..UPDATES {
                    let acquired = acquired.clone();
                    let updated = updated.clone();
                    let dropped = dropped.clone();
                    dispatch_scoped_resource(
                        |task| sender.send(task),
                        move || {
                            assert_eq!(thread::current().id(), owner);
                            acquired.fetch_add(1, Ordering::SeqCst);
                            Some(ThreadBoundResource {
                                owner,
                                drops: dropped,
                                _not_send: Rc::new(()),
                            })
                        },
                        move |resource| {
                            assert_eq!(thread::current().id(), resource.owner);
                            updated.fetch_add(1, Ordering::SeqCst);
                        },
                    )
                    .expect("queue owner-thread task");
                }
            }));
        }
        for worker in workers {
            worker
                .join()
                .expect("worker finishes without waiting for UI");
        }
        drop(sender);
        assert_eq!(acquired.load(Ordering::SeqCst), 0);
        for task in receiver {
            task();
        }
        assert_eq!(acquired.load(Ordering::SeqCst), WORKERS * UPDATES);
        assert_eq!(updated.load(Ordering::SeqCst), WORKERS * UPDATES);
        assert_eq!(dropped.load(Ordering::SeqCst), WORKERS * UPDATES);
    }

    #[test]
    fn removed_tray_is_a_noop() {
        let result = dispatch_scoped_resource(
            |task| {
                task();
                Ok::<_, ()>(())
            },
            || None::<Rc<()>>,
            |_| panic!("a missing tray must not be updated"),
        );
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn dispatch_failure_never_acquires_a_native_handle() {
        let result = dispatch_scoped_resource(
            |_task| Err("event loop closed"),
            || -> Option<Rc<()>> { panic!("lookup must be deferred until dispatch") },
            |_| panic!("an undispatched task must not run"),
        );
        assert_eq!(result, Err("event loop closed"));
    }
}
