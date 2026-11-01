use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{path::Path, sync::Arc};
use tokio::sync::{Mutex, oneshot};

use super::LibOverride;
use crate::{ast::ASTBuilder, span::FileStorage};

#[derive(Debug)]
pub struct ParseManager {
    active_tasks: AtomicUsize,
    done_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub file_storage: Mutex<FileStorage<ASTBuilder>>,
    /// `ParseOptions::lib_override`, in order: the file name (the last
    /// component of the path; `None` when the path has none, which matches
    /// no call), the section to replace and the section to load, all in
    /// lowercase.
    lib_override: Vec<(Option<String>, String, String)>,
    /// One flag for each entry of `lib_override`: set when it changed the
    /// section of a `.lib` call.
    lib_override_matched: Vec<AtomicBool>,
}

/// The last component of `path`, in lowercase.
fn file_name_lowercase(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

impl ParseManager {
    /// A manager with no `.lib` override.
    #[cfg(test)]
    pub(super) fn new() -> (Arc<Self>, oneshot::Receiver<()>) {
        Self::with_lib_override(&[])
    }

    /// A manager whose `.lib` calls use `lib_override`
    /// (see `ParseOptions::lib_override`).
    pub(super) fn with_lib_override(
        lib_override: &[LibOverride],
    ) -> (Arc<Self>, oneshot::Receiver<()>) {
        let (tx, done_rx) = oneshot::channel();
        let lib_override = lib_override
            .iter()
            .map(|o| {
                (
                    file_name_lowercase(&o.file),
                    o.from.to_lowercase(),
                    o.to.to_lowercase(),
                )
            })
            .collect::<Vec<_>>();
        let lib_override_matched = lib_override
            .iter()
            .map(|_| AtomicBool::new(false))
            .collect();
        (
            Arc::new(ParseManager {
                active_tasks: AtomicUsize::new(0),
                done_tx: Mutex::new(Some(tx)),
                file_storage: Mutex::new(FileStorage::default()),
                lib_override,
                lib_override_matched,
            }),
            done_rx,
        )
    }

    /// The section that `.lib '<file_name>' <section>` loads (`section` is
    /// in lowercase): the `to` section of the first override whose file
    /// name is the file name of `file_name` (without its directory, in any
    /// letter case) and whose `from` section is `section`, else `section`.
    /// A call in the library itself (a section that calls another section
    /// of its own file) is a call like any other.
    pub(super) fn lib_section(&self, file_name: &Path, section: String) -> String {
        if self.lib_override.is_empty() {
            return section;
        }
        let Some(name) = file_name_lowercase(file_name) else {
            return section;
        };
        let found = self
            .lib_override
            .iter()
            .position(|(f, from, _)| f.as_deref() == Some(&name) && *from == section);
        match found {
            Some(k) => {
                self.lib_override_matched[k].store(true, Ordering::Relaxed);
                self.lib_override[k].2.clone()
            }
            None => section,
        }
    }

    /// For each entry of `ParseOptions::lib_override`, in order: `true`
    /// when it changed the section of a `.lib` call.
    pub(super) fn lib_override_matched(&self) -> Vec<bool> {
        self.lib_override_matched
            .iter()
            .map(|m| m.load(Ordering::Relaxed))
            .collect()
    }

    /// start a new async parse task
    pub fn spawn_parse<F>(self: &Arc<Self>, fut: F)
    where
        F: core::future::Future<Output = ()> + Send + 'static,
    {
        // 1. task num +1
        self.active_tasks.fetch_add(1, Ordering::SeqCst);

        // 2. clone Arc<ParseManager>, move it into async task
        let manager_clone = self.clone();

        // 3. start task
        crate::spawn(async move {
            // waiting for parser done
            fut.await;
            // task_done
            manager_clone.task_done().await;
        });
    }
    pub(super) async fn wait(self: &Arc<Self>, done_rx: oneshot::Receiver<()>) {
        if 0 != self.active_tasks.load(Ordering::SeqCst) {
            let result = done_rx.await;
            assert!(
                result.is_ok(),
                "The single task should eventually trigger done signal"
            );
        }
    }
    /// internal, record one taks is done
    async fn task_done(&self) {
        let remaining = self.active_tasks.fetch_sub(1, Ordering::SeqCst) - 1;
        if remaining == 0 {
            if let Some(tx) = self.done_tx.lock().await.take() {
                _ = tx.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn test_parse_manager_multiple_tasks() {
        let (manager, done_rx) = ParseManager::new();

        manager.spawn_parse(async {
            for i in 0..3 {
                sleep(Duration::from_millis(50)).await;
                println!("task1-{i}");
            }
            println!("done"); // do something...
        });

        manager.spawn_parse(async {
            for i in 0..5 {
                sleep(Duration::from_millis(30)).await;
                println!("task2-{i}");
            }
            println!("done");
            // do something...
        });

        manager.wait(done_rx).await;
    }

    #[tokio::test]
    async fn test_parse_manager_no_tasks() {
        let (manager, done_rx) = ParseManager::new();
        manager.wait(done_rx).await;
    }

    #[tokio::test]
    async fn test_parse_manager_one_task() {
        let (manager, done_rx) = ParseManager::new();

        manager.spawn_parse(async {
            sleep(Duration::from_millis(10)).await;
            // do parse
        });
        manager.wait(done_rx).await;
    }
}
