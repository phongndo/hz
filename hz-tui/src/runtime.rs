use std::{future::Future, io, sync::OnceLock, thread};

use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::oneshot,
};

pub(crate) fn build_runtime() -> io::Result<Runtime> {
    Builder::new_multi_thread().enable_all().build()
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
    let _ = thread::Builder::new()
        .name("hz-blocking".to_owned())
        .spawn(function);
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
