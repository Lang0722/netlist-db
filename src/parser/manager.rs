use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{
    collections::HashMap,
    mem,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Mutex, OnceCell, oneshot};

use super::ParseOptions;
use crate::{
    ast::ASTBuilder,
    err::ParseError,
    span::{FileStorage, ParsedId, Pos},
};

#[derive(Debug)]
pub struct ParseManager {
    active_tasks: AtomicUsize,
    done_tx: StdMutex<Option<oneshot::Sender<()>>>,
    pub file_storage: Mutex<FileStorage<ASTBuilder>>,
    /// The text of each library file whose sections are loaded (read once
    /// for all its sections), and its first line that is not UTF-8; or why it
    /// could not be read
    pub(super) lib_text: Mutex<HashMap<PathBuf, Arc<OnceCell<SharedText>>>>,
    /// `ParseOptions::lib_override`, in order: the file name (the last
    /// component of the path; `None` when the path has none, which matches
    /// no call), the section to replace and the section to load, all in
    /// lowercase.
    lib_override: Vec<(Option<String>, String, String)>,
    /// One flag for each entry of `lib_override`: set when it changed the
    /// section of a `.lib` call.
    lib_override_matched: Vec<AtomicBool>,
    /// `ParseOptions::skip_prot`
    skip_prot: bool,
    /// Where a relative include is looked for, after the folder of the file
    /// that includes it: the folders of the top files, then the working folder
    pub(super) roots: Vec<PathBuf>,
    /// The includes not found so far, which wait for all of `.option search`
    deferred: StdMutex<Vec<Include>>,
}

/// An `.inc` or `.lib` line whose file is to be read
#[derive(Debug)]
pub(super) struct Include {
    /// the name on the line
    pub(super) file_name: PathBuf,
    /// the path of the file
    pub(super) path: PathBuf,
    pub(super) section: Option<String>,
    pub(super) pos: Option<Pos>,
    pub(super) res: Arc<std::sync::OnceLock<Result<ParsedId, ParseError>>>,
}

pub(super) type SharedText = Result<Arc<(String, Option<u32>)>, (std::io::ErrorKind, String)>;

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

/// The last component of `path`, in lowercase.
fn file_name_lowercase(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

impl ParseManager {
    /// A manager with the default options.
    #[cfg(test)]
    pub(super) fn new() -> (Arc<Self>, oneshot::Receiver<()>) {
        Self::with_options(&ParseOptions::default(), Vec::new())
    }

    /// A manager whose parse uses `opts`, with the folders `roots` for includes.
    pub(super) fn with_options(
        opts: &ParseOptions,
        roots: Vec<PathBuf>,
    ) -> (Arc<Self>, oneshot::Receiver<()>) {
        let (tx, done_rx) = oneshot::channel();
        let lib_override = opts
            .lib_override
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
                done_tx: StdMutex::new(Some(tx)),
                file_storage: Mutex::new(FileStorage::default()),
                lib_text: Mutex::new(HashMap::new()),
                lib_override,
                lib_override_matched,
                skip_prot: opts.skip_prot,
                roots,
                deferred: StdMutex::new(Vec::new()),
            }),
            done_rx,
        )
    }

    /// `ParseOptions::skip_prot`
    pub(super) fn skip_prot(&self) -> bool {
        self.skip_prot
    }

    /// Keeps `include`, whose file is not found so far.
    pub(super) fn defer(&self, include: Include) {
        self.deferred
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(include);
    }

    /// The includes kept by [`ParseManager::defer`]
    pub(super) fn take_deferred(&self) -> Vec<Include> {
        mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// A new signal for [`ParseManager::wait`], for work that starts after the
    /// count reached zero. The caller holds a guard while it starts this work.
    pub(super) fn rearm(&self) -> oneshot::Receiver<()> {
        let (tx, done_rx) = oneshot::channel();
        *self
            .done_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(tx);
        done_rx
    }

    /// Register one unit of work, released when the guard is dropped.
    pub(super) fn task_guard(self: &Arc<Self>) -> TaskGuard {
        self.active_tasks.fetch_add(1, Ordering::SeqCst);
        TaskGuard(self.clone())
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
