use core::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Mutex, OnceCell, oneshot};

use crate::{ast::ASTBuilder, span::FileStorage};

#[derive(Debug)]
pub struct ParseManager {
    active_tasks: AtomicUsize,
    done_tx: StdMutex<Option<oneshot::Sender<()>>>,
    pub file_storage: Mutex<FileStorage<ASTBuilder>>,
    /// The text of each library file whose sections are loaded (read once
    /// for all its sections), and its encoding warning; or why it could not
    /// be read
    pub(super) lib_text: Mutex<HashMap<PathBuf, Arc<OnceCell<SharedText>>>>,
}

pub(super) type SharedText = Result<Arc<(String, Option<String>)>, (std::io::ErrorKind, String)>;

/// One unit of outstanding work: [`ParseManager::wait`] does not return while
/// a guard is alive. It is released on drop, so a task that panics still
/// releases it instead of making `wait` hang.
#[derive(Debug)]
pub(super) struct TaskGuard(Arc<ParseManager>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.task_done();
    }
}

impl ParseManager {
    pub(super) fn new() -> (Arc<Self>, oneshot::Receiver<()>) {
        let (tx, done_rx) = oneshot::channel();
        (
            Arc::new(ParseManager {
                active_tasks: AtomicUsize::new(0),
                done_tx: StdMutex::new(Some(tx)),
                file_storage: Mutex::new(FileStorage::default()),
                lib_text: Mutex::new(HashMap::new()),
            }),
            done_rx,
        )
    }

    /// Register one unit of work, released when the guard is dropped.
    pub(super) fn task_guard(self: &Arc<Self>) -> TaskGuard {
        self.active_tasks.fetch_add(1, Ordering::SeqCst);
        TaskGuard(self.clone())
    }

    /// start a new async parse task
    pub fn spawn_parse<F>(self: &Arc<Self>, fut: F)
    where
        F: core::future::Future<Output = ()> + Send + 'static,
    {
        // Count the task before spawning it, while the caller (which holds its
        // own guard) is still running, so the count cannot drop to zero early.
        let guard = self.task_guard();
        crate::spawn(async move {
            let _guard = guard;
            fut.await;
        });
    }

    /// Wait until every [`TaskGuard`] is dropped.
    ///
    /// The completion signal fires the first time the count drops to zero, so
    /// the caller must hold a guard while it starts the first tasks; otherwise
    /// they could all finish before the next one is started.
    pub(super) async fn wait(&self, done_rx: oneshot::Receiver<()>) {
        if 0 != self.active_tasks.load(Ordering::SeqCst) {
            let result = done_rx.await;
            assert!(
                result.is_ok(),
                "The single task should eventually trigger done signal"
            );
        }
    }

    /// internal, record one task is done
    fn task_done(&self) {
        if self.active_tasks.fetch_sub(1, Ordering::SeqCst) == 1 {
            let mut done_tx = self
                .done_tx
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(tx) = done_tx.take() {
                _ = tx.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicBool;
    use tokio::time::{Duration, sleep, timeout};

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

    /// A task spawned after all earlier tasks finished must still be awaited,
    /// as long as the spawner held a guard (this is what top-level files do).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_wait_covers_tasks_spawned_after_others_finished() {
        let (manager, done_rx) = ParseManager::new();
        let finished = Arc::new(AtomicBool::new(false));
        let top_guard = manager.task_guard();
        {
            let manager = manager.clone();
            let finished = finished.clone();
            crate::spawn(async move {
                // finishes right away: without the guard the count hits zero here
                manager.spawn_parse(async {});
                sleep(Duration::from_millis(50)).await;
                manager.spawn_parse(async move {
                    sleep(Duration::from_millis(50)).await;
                    finished.store(true, Ordering::SeqCst);
                });
            })
            .await
            .unwrap();
        }
        drop(top_guard);
        manager.wait(done_rx).await;
        assert!(finished.load(Ordering::SeqCst));
    }

    /// A panicking task must not leave `wait` hanging forever.
    #[tokio::test]
    async fn test_panicking_task_does_not_hang_wait() {
        let (manager, done_rx) = ParseManager::new();
        manager.spawn_parse(async { panic!("simulated parser bug") });
        timeout(Duration::from_secs(10), manager.wait(done_rx))
            .await
            .expect("wait() hung after a task panicked");
    }
}
