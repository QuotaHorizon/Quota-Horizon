use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// A rotating refresh token must have only one renewal in flight, including
/// requests from different WebViews and the local proxy. Callers re-read the
/// stored credential after acquiring this lock, then retain their CAS write.
pub(crate) fn for_managed_auth(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_account_shares_renewal_lock_but_other_accounts_do_not() {
        let first = for_managed_auth(Path::new("/synthetic/account-a/auth.json"));
        let second = for_managed_auth(Path::new("/synthetic/account-a/auth.json"));
        let other = for_managed_auth(Path::new("/synthetic/account-b/auth.json"));
        assert!(Arc::ptr_eq(&first, &second));
        let held = first.lock().unwrap();
        assert!(second.try_lock().is_err());
        assert!(other.try_lock().is_ok());
        drop(held);
        assert!(second.try_lock().is_ok());
    }
}
