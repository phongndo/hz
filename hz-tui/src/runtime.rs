use std::{future::Future, io, sync::OnceLock, thread, time::Duration};

use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::{mpsc, oneshot},
};

const RUNTIME_WORKER_THREADS: usize = 2;
const RUNTIME_MAX_BLOCKING_THREADS: usize = 4;
const CHANNEL_SEND_TIMEOUT: Duration = Duration::from_millis(10);

pub(crate) fn build_runtime() -> io::Result<Runtime> {
    Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKER_THREADS)
        .max_blocking_threads(RUNTIME_MAX_BLOCKING_THREADS)
        .thread_name("hz-tokio")
        .enable_time()
        .build()
}

pub(crate) fn block_on<F, R>(future: F) -> io::Result<R>
where
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    if Handle::try_current().is_ok() {
        let handle = thread::Builder::new()
            .name("hz-runtime".to_owned())
            .spawn(move || {
                let runtime = build_runtime()?;
                Ok(runtime.block_on(future))
            })?;
        return handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    }

    let runtime = build_runtime()?;
    Ok(runtime.block_on(future))
}

pub(crate) fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(future)
    } else {
        global_runtime().spawn(future)
    }
}

pub(crate) fn spawn_blocking<F, R>(function: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn_blocking(function)
    } else {
        global_runtime().spawn_blocking(function)
    }
}

pub(crate) fn spawn_detached_blocking<F>(function: F)
where
    F: FnOnce() + Send + 'static,
{
    drop(spawn_blocking(function));
}

pub(crate) async fn run_detached_blocking<F, R>(function: F) -> Result<R, oneshot::error::RecvError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    spawn_detached_blocking(move || {
        let _ = tx.send(function());
    });
    rx.await
}

pub(crate) fn send_with_timeout<T>(sender: &mpsc::Sender<T>, mut value: T) -> bool {
    match sender.try_send(value) {
        Ok(()) => return true,
        Err(mpsc::error::TrySendError::Full(next_value)) => value = next_value,
        Err(mpsc::error::TrySendError::Closed(_)) => return false,
    }

    let start = std::time::Instant::now();
    loop {
        if start.elapsed() >= CHANNEL_SEND_TIMEOUT {
            return false;
        }

        thread::sleep(Duration::from_millis(1));
        match sender.try_send(value) {
            Ok(()) => return true,
            Err(mpsc::error::TrySendError::Full(next_value)) => value = next_value,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
        }
    }
}

pub(crate) async fn send_async_with_timeout<T>(sender: &mpsc::Sender<T>, value: T) -> bool {
    let value = match sender.try_send(value) {
        Ok(()) => return true,
        Err(mpsc::error::TrySendError::Full(value)) => value,
        Err(mpsc::error::TrySendError::Closed(_)) => return false,
    };

    matches!(
        tokio::time::timeout(CHANNEL_SEND_TIMEOUT, sender.send(value)).await,
        Ok(Ok(()))
    )
}

fn global_runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| build_runtime().expect("tokio runtime should start"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_without_current_tokio_runtime() {
        let value = block_on(async { 42 }).expect("runtime should start");

        assert_eq!(value, 42);
    }

    #[test]
    fn block_on_runs_inside_current_tokio_runtime() {
        let runtime = build_runtime().expect("runtime should start");
        let value = runtime.block_on(async {
            block_on(async { 42 }).expect("nested runtime helper should run on a thread")
        });

        assert_eq!(value, 42);
    }
}
