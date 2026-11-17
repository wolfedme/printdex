//! Bridge from async code to the rayon CPU pool.

/// Runs `f` on the rayon pool and awaits its result without blocking a tokio
/// worker. A panic inside `f` is resumed on the awaiting task.
pub async fn run_cpu<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    rayon::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        // The receiver is gone only if the awaiting task was dropped.
        let _ = tx.send(result);
    });
    match rx.await {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => unreachable!("rayon job dropped its result sender"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_cpu_returns_value() {
        assert_eq!(run_cpu(|| 2 + 2).await, 4);
    }

    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn run_cpu_propagates_panic() {
        run_cpu(|| -> () { panic!("boom") }).await;
    }
}
