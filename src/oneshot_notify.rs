use std::sync::{Condvar, Mutex, atomic::AtomicBool};

use crate::prelude::*;
use tokio::sync::Notify;

/// Like a notify that keeps returning true once it has been notified
#[derive(Debug)]
pub struct OneshotNotify {
    flag: AtomicBool,
    async_notify: Notify,
    sync_notify: (Mutex<bool>, Condvar),
}

impl OneshotNotify {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            async_notify: Notify::new(),
            sync_notify: (Mutex::new(false), Condvar::new()),
        }
    }

    pub fn notify(&self) {
        if !self.flag.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let (lock, cvar) = &self.sync_notify;
            let mut notified = lock.lock().unwrap();
            *notified = true;
            // sync notify
            cvar.notify_all();
            // async notify
            self.async_notify.notify_waiters();
        }
    }

    pub async fn wait(&self) {
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        info!("Waiting for notification");
        self.async_notify.notified().await;
        info!("Notification received");
    }

    pub fn wait_blocking(&self) {
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // wait on the condition variable
        info!("Waiting for notification");
        let (lock, cvar) = &self.sync_notify;
        let mut notified = lock.lock().unwrap();
        while !*notified {
            notified = cvar.wait(notified).unwrap();
        }
        info!("Notification received");
    }
}
