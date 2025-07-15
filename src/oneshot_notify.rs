use std::sync::{Condvar, Mutex, atomic::AtomicBool};

use crate::prelude::*;
use tokio::sync::Notify;

/// Like a notify that keeps returning true once it has been notified
#[derive(Debug)]
pub struct OneshotNotify {
    flag: AtomicBool,
    notify: Notify,
    condvar: (Mutex<bool>, Condvar),
}

impl OneshotNotify {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            condvar: (Mutex::new(false), Condvar::new()),
        }
    }

    pub fn notify(&self) {
        if !self.flag.swap(true, std::sync::atomic::Ordering::AcqRel) {
            let (lock, cvar) = &self.condvar;
            let mut notified = lock.lock().unwrap();
            *notified = true;
            // sync notify
            cvar.notify_all();
            // async notify
            self.notify.notify_waiters();
        }
    }

    pub async fn wait(&self) {
        info!("Waiting for egui ctx");
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        self.notify.notified().await;
        info!("Egui ctx is available now");
    }

    pub fn wait_blocking(&self) {
        // if already notified once, return immediately
        if self.flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        // wait on the condition variable
        let (lock, cvar) = &self.condvar;
        let mut notified = lock.lock().unwrap();
        while !*notified {
            notified = cvar.wait(notified).unwrap();
        }
    }
}
